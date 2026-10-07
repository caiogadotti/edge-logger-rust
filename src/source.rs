//! Fontes de leituras. Cada uma roda numa thread e empurra para o canal limitado.

use crate::model::Leitura;
use anyhow::{Context, Result};
use crossbeam_channel::{Sender, TrySendError};
use rand_distr::{Distribution, Normal};
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Contadores {
    pub recebidas: AtomicU64,
    pub descartadas: AtomicU64,
    pub gravadas: AtomicU64,
    pub bytes_disco: AtomicU64,
    pub parar: AtomicBool,
}

impl Contadores {
    pub fn novo() -> Arc<Self> {
        Arc::new(Self {
            recebidas: AtomicU64::new(0),
            descartadas: AtomicU64::new(0),
            gravadas: AtomicU64::new(0),
            bytes_disco: AtomicU64::new(0),
            parar: AtomicBool::new(false),
        })
    }
}

/// Envia respeitando RN-03: bloqueia (backpressure) ou descarta contando.
pub fn enviar(tx: &Sender<Leitura>, l: Leitura, descartar: bool, c: &Contadores) -> bool {
    c.recebidas.fetch_add(1, Ordering::Relaxed);
    if descartar {
        match tx.try_send(l) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                c.descartadas.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    } else {
        tx.send(l).is_ok()
    }
}

pub fn agora_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64
}

pub struct ConfigSim {
    pub sensores: u16,
    pub taxa: u64,
    pub total: Option<u64>,
    pub semente_ts: Option<u64>,
}

/// Senoide por sensor + ruído gaussiano + pico de falha raro (0,01%).
pub struct Simulador {
    cfg: ConfigSim,
    ruido: Normal<f64>,
    i: u64,
}

impl Simulador {
    pub fn novo(cfg: ConfigSim) -> Self {
        Self {
            cfg,
            ruido: Normal::new(0.0, 0.5).unwrap(),
            i: 0,
        }
    }

    pub fn proxima(&mut self, rng: &mut impl rand::Rng, base_ts: u64) -> Leitura {
        let s = (self.i % self.cfg.sensores as u64) as u16;
        let passo_us = (1_000_000 * self.cfg.sensores as u64 / self.cfg.taxa.max(1)).max(1);
        let ts = base_ts + (self.i / self.cfg.sensores as u64) * passo_us;
        let t = ts as f64 / 1e6;
        let mut v = 220.0 * (2.0 * std::f64::consts::PI * (0.5 + s as f64 * 0.1) * t).sin()
            + self.ruido.sample(rng);
        if rng.gen_ratio(1, 10_000) {
            v *= 3.0;
        }
        self.i += 1;
        Leitura {
            sensor_id: s,
            timestamp_us: ts,
            valor: (v * 100.0).round() / 100.0,
        }
    }
}

pub fn rodar_simulador(cfg: ConfigSim, tx: Sender<Leitura>, descartar: bool, c: Arc<Contadores>) {
    let taxa = cfg.taxa.max(1);
    let total = cfg.total;
    let base = cfg.semente_ts.unwrap_or_else(agora_us);
    let mut sim = Simulador::novo(cfg);
    let mut rng = rand::thread_rng();
    let inicio = Instant::now();
    let mut enviadas = 0u64;
    while !c.parar.load(Ordering::Relaxed) && total.is_none_or(|t| enviadas < t) {
        // envia em rajadas de 1 ms para manter a taxa sem um sleep por leitura
        let devidas = (inicio.elapsed().as_secs_f64() * taxa as f64) as u64;
        if enviadas >= devidas {
            std::thread::sleep(Duration::from_micros(200));
            continue;
        }
        let lote = (devidas - enviadas).min(total.map_or(u64::MAX, |t| t - enviadas));
        for _ in 0..lote {
            if !enviar(&tx, sim.proxima(&mut rng, base), descartar, &c) {
                return;
            }
        }
        enviadas += lote;
    }
}

/// JSON por linha: {"sensor_id":1,"timestamp_us":123,"valor":4.5}; timestamp opcional (usa o relógio).
pub fn rodar_stdin(tx: Sender<Leitura>, descartar: bool, c: Arc<Contadores>) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Entrada {
        sensor_id: u16,
        timestamp_us: Option<u64>,
        valor: f64,
    }
    for linha in std::io::stdin().lock().lines() {
        let linha = linha?;
        if linha.trim().is_empty() {
            continue;
        }
        let e: Entrada =
            serde_json::from_str(&linha).with_context(|| format!("linha inválida: {linha}"))?;
        let l = Leitura {
            sensor_id: e.sensor_id,
            timestamp_us: e.timestamp_us.unwrap_or_else(agora_us),
            valor: e.valor,
        };
        if !enviar(&tx, l, descartar, &c) {
            break;
        }
    }
    Ok(())
}

/// Assina um tópico MQTT; payload JSON igual ao do stdin (um objeto ou uma lista).
pub fn rodar_mqtt(
    host: &str,
    porta: u16,
    topico: &str,
    tx: Sender<Leitura>,
    descartar: bool,
    c: Arc<Contadores>,
) -> Result<()> {
    use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Payload {
        Uma(Entrada),
        Varias(Vec<Entrada>),
    }
    #[derive(serde::Deserialize)]
    struct Entrada {
        sensor_id: u16,
        timestamp_us: Option<u64>,
        valor: f64,
    }

    let mut opts = MqttOptions::new(format!("edgelog-{}", std::process::id()), host, porta);
    opts.set_keep_alive(Duration::from_secs(10));
    opts.set_max_packet_size(1 << 20, 1 << 20);
    let (cliente, mut conexao) = Client::new(opts, 10_000);
    cliente.subscribe(topico, QoS::AtLeastOnce)?;

    for evento in conexao.iter() {
        if c.parar.load(Ordering::Relaxed) {
            break;
        }
        match evento {
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let entradas = match serde_json::from_slice::<Payload>(&p.payload) {
                    Ok(Payload::Uma(e)) => vec![e],
                    Ok(Payload::Varias(v)) => v,
                    Err(e) => {
                        eprintln!("payload MQTT ignorado: {e}");
                        continue;
                    }
                };
                for e in entradas {
                    let l = Leitura {
                        sensor_id: e.sensor_id,
                        timestamp_us: e.timestamp_us.unwrap_or_else(agora_us),
                        valor: e.valor,
                    };
                    if !enviar(&tx, l, descartar, &c) {
                        return Ok(());
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("MQTT: {e} (tentando de novo)");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    Ok(())
}
