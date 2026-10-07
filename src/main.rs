mod codec;
mod model;
mod recorder;
mod segment;
mod source;
mod wal;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};
use model::Leitura;
use recorder::{ConfigGravacao, Gravador};
use segment::{ler_segmento, listar_segmentos};
use source::{ConfigSim, Contadores};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "edgelog",
    version,
    about = "Logger de telemetria de borda em Rust"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Fonte {
    Sim,
    Stdin,
    Mqtt,
}

#[derive(Clone, Copy, ValueEnum)]
enum Formato {
    Csv,
    Json,
}

#[derive(Subcommand)]
enum Cmd {
    /// Recebe leituras e grava em disco
    Record {
        #[arg(long, value_enum, default_value = "stdin")]
        source: Fonte,
        #[arg(long, default_value = "data")]
        dir: PathBuf,
        /// arquivo TOML com as opções de gravação (as flags têm prioridade)
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        block: Option<usize>,
        #[arg(long)]
        fsync_ms: Option<u64>,
        /// desliga o fsync (só para benchmark de CPU)
        #[arg(long)]
        no_fsync: bool,
        /// descarta (e conta) em vez de segurar a fonte quando a fila enche
        #[arg(long)]
        drop_on_full: bool,
        #[arg(long, default_value_t = 262_144)]
        queue: usize,
        #[arg(long, default_value_t = 5)]
        status_s: u64,
        // fonte sim
        #[arg(long, default_value_t = 100_000)]
        rate: u64,
        #[arg(long, default_value_t = 8)]
        sensors: u16,
        #[arg(long)]
        count: Option<u64>,
        // fonte mqtt
        #[arg(long, default_value = "localhost")]
        mqtt_host: String,
        #[arg(long, default_value_t = 1883)]
        mqtt_port: u16,
        #[arg(long, default_value = "sensores/#")]
        mqtt_topic: String,
    },
    /// Gera sensores sintéticos em JSON por linha (para encadear com `record`)
    Simulate {
        #[arg(long, default_value_t = 100_000)]
        rate: u64,
        #[arg(long, default_value_t = 8)]
        sensors: u16,
        #[arg(long)]
        count: Option<u64>,
    },
    /// Exporta as leituras gravadas
    Read {
        #[arg(long, default_value = "data")]
        dir: PathBuf,
        #[arg(long, value_enum, default_value = "csv")]
        format: Formato,
        #[arg(long)]
        sensor: Option<u16>,
        /// início (timestamp em µs)
        #[arg(long)]
        from: Option<u64>,
        /// fim (timestamp em µs)
        #[arg(long)]
        to: Option<u64>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Confere a integridade dos segmentos e do WAL
    Verify {
        #[arg(long, default_value = "data")]
        dir: PathBuf,
    },
    /// Mede vazão de gravação e taxa de compressão
    Bench {
        #[arg(long, default_value_t = 5_000_000)]
        count: u64,
        #[arg(long, default_value_t = 8)]
        sensors: u16,
        #[arg(long)]
        no_fsync: bool,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Record {
            source,
            dir,
            config,
            block,
            fsync_ms,
            no_fsync,
            drop_on_full,
            queue,
            status_s,
            rate,
            sensors,
            count,
            mqtt_host,
            mqtt_port,
            mqtt_topic,
        } => {
            let mut cfg: ConfigGravacao = match config {
                Some(p) => toml::from_str(&std::fs::read_to_string(p)?)?,
                None => ConfigGravacao::default(),
            };
            cfg.dir = dir;
            if let Some(b) = block {
                cfg.bloco = b;
            }
            if let Some(f) = fsync_ms {
                cfg.fsync_ms = f;
            }
            if no_fsync {
                cfg.fsync = false;
            }
            gravar(
                cfg,
                source,
                drop_on_full,
                queue,
                status_s,
                ConfigSim {
                    sensores: sensors,
                    taxa: rate,
                    total: count,
                    semente_ts: None,
                },
                (mqtt_host, mqtt_port, mqtt_topic),
            )
        }
        Cmd::Simulate {
            rate,
            sensors,
            count,
        } => simular(rate, sensors, count),
        Cmd::Read {
            dir,
            format,
            sensor,
            from,
            to,
            out,
        } => ler(dir, format, sensor, from, to, out),
        Cmd::Verify { dir } => verificar(dir),
        Cmd::Bench {
            count,
            sensors,
            no_fsync,
            json,
        } => bench(count, sensors, !no_fsync, json),
    }
}

fn gravar(
    cfg: ConfigGravacao,
    fonte: Fonte,
    descartar: bool,
    fila: usize,
    status_s: u64,
    sim: ConfigSim,
    mqtt: (String, u16, String),
) -> Result<()> {
    let c = Contadores::novo();
    let (gravador, partida) = Gravador::abrir(cfg.clone(), c.clone())?;
    if partida.recuperadas_wal > 0 || partida.bytes_truncados > 0 {
        eprintln!(
            "recuperação: {} leituras salvas do WAL, {} bytes incompletos descartados",
            partida.recuperadas_wal, partida.bytes_truncados
        );
    }
    let base_gravadas = c.gravadas.load(Ordering::Relaxed);

    let (tx, rx) = crossbeam_channel::bounded::<Leitura>(fila);
    let cg = c.clone();
    let t_fonte = std::thread::spawn(move || -> Result<()> {
        match fonte {
            Fonte::Sim => source::rodar_simulador(sim, tx, descartar, cg),
            Fonte::Stdin => source::rodar_stdin(tx, descartar, cg)?,
            Fonte::Mqtt => source::rodar_mqtt(&mqtt.0, mqtt.1, &mqtt.2, tx, descartar, cg)?,
        }
        Ok(())
    });

    let cs = c.clone();
    let rx_status = rx.clone();
    std::thread::spawn(move || {
        let mut ant = 0u64;
        loop {
            std::thread::sleep(Duration::from_secs(status_s.max(1)));
            let rec = cs.recebidas.load(Ordering::Relaxed);
            eprintln!(
                "[edgelog] {:>10.0} leit/s | gravadas {:>12} | disco {:>8.2} MB | fila {:>7} | descartes {}",
                (rec - ant) as f64 / status_s.max(1) as f64,
                cs.gravadas.load(Ordering::Relaxed),
                cs.bytes_disco.load(Ordering::Relaxed) as f64 / 1e6,
                rx_status.len(),
                cs.descartadas.load(Ordering::Relaxed),
            );
            ant = rec;
        }
    });

    let inicio = Instant::now();
    gravador.rodar(rx)?;
    t_fonte.join().expect("thread da fonte caiu")?;
    let gravadas = c.gravadas.load(Ordering::Relaxed) - base_gravadas;
    let s = inicio.elapsed().as_secs_f64();
    eprintln!(
        "fim: {gravadas} leituras em {s:.2}s ({:.0} leit/s), {} descartes",
        gravadas as f64 / s,
        c.descartadas.load(Ordering::Relaxed)
    );
    Ok(())
}

fn simular(taxa: u64, sensores: u16, total: Option<u64>) -> Result<()> {
    let mut sim = source::Simulador::novo(ConfigSim {
        sensores,
        taxa,
        total,
        semente_ts: None,
    });
    let mut rng = rand::thread_rng();
    let base = source::agora_us();
    let inicio = Instant::now();
    let mut out = BufWriter::with_capacity(1 << 20, std::io::stdout().lock());
    let mut n = 0u64;
    while total.is_none_or(|t| n < t) {
        let devidas = (inicio.elapsed().as_secs_f64() * taxa as f64) as u64;
        if n >= devidas {
            out.flush()?;
            std::thread::sleep(Duration::from_micros(200));
            continue;
        }
        for _ in 0..(devidas - n).min(total.map_or(u64::MAX, |t| t - n)) {
            let l = sim.proxima(&mut rng, base);
            if writeln!(
                out,
                r#"{{"sensor_id":{},"timestamp_us":{},"valor":{}}}"#,
                l.sensor_id, l.timestamp_us, l.valor
            )
            .is_err()
            {
                return Ok(()); // pipe fechado
            }
            n += 1;
        }
    }
    out.flush()?;
    Ok(())
}

fn carregar(
    dir: &std::path::Path,
    sensor: Option<u16>,
    de: Option<u64>,
    ate: Option<u64>,
) -> Result<Vec<Leitura>> {
    let (de, ate) = (de.unwrap_or(0), ate.unwrap_or(u64::MAX));
    let mut tudo = Vec::new();
    for p in listar_segmentos(dir)? {
        let v = ler_segmento(&p, &|b| b.ts_max >= de && b.ts_min <= ate)?;
        tudo.extend(v.leituras.into_iter().filter(|l| {
            sensor.is_none_or(|s| l.sensor_id == s) && l.timestamp_us >= de && l.timestamp_us <= ate
        }));
    }
    let wal = dir.join("wal.log");
    if wal.exists() {
        // leitura somente: não trunca o WAL de um gravador que pode estar rodando
        let bytes = std::fs::read(&wal)?;
        tudo.extend(wal::varrer_publico(&bytes).into_iter().filter(|l| {
            sensor.is_none_or(|s| l.sensor_id == s) && l.timestamp_us >= de && l.timestamp_us <= ate
        }));
    }
    tudo.sort_by_key(|l| (l.timestamp_us, l.sensor_id));
    Ok(tudo)
}

fn ler(
    dir: PathBuf,
    formato: Formato,
    sensor: Option<u16>,
    de: Option<u64>,
    ate: Option<u64>,
    saida: Option<PathBuf>,
) -> Result<()> {
    let ls = carregar(&dir, sensor, de, ate)?;
    let destino: Box<dyn Write> = match saida {
        Some(p) => Box::new(std::fs::File::create(p)?),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut out = BufWriter::with_capacity(1 << 20, destino);
    match formato {
        Formato::Csv => {
            writeln!(out, "sensor_id,timestamp_us,valor")?;
            for l in &ls {
                writeln!(out, "{},{},{}", l.sensor_id, l.timestamp_us, l.valor)?;
            }
        }
        Formato::Json => {
            for l in &ls {
                serde_json::to_writer(&mut out, l)?;
                writeln!(out)?;
            }
        }
    }
    out.flush()?;
    eprintln!("{} leituras exportadas", ls.len());
    Ok(())
}

fn verificar(dir: PathBuf) -> Result<()> {
    let segs = listar_segmentos(&dir)?;
    if segs.is_empty() {
        bail!("nenhum segmento em {}", dir.display());
    }
    let (mut blocos, mut ruins, mut leituras) = (0, 0, 0u64);
    for p in &segs {
        let v = ler_segmento(p, &|_| false)?;
        let r = v.blocos.iter().filter(|b| !b.ok).count();
        let n: u64 = v.blocos.iter().filter(|b| b.ok).map(|b| b.n as u64).sum();
        println!(
            "{}  blocos {:>5}  corrompidos {:>3}  leituras {:>12}{}",
            p.file_name().unwrap().to_string_lossy(),
            v.blocos.len(),
            r,
            n,
            if v.bytes_lixo_final > 0 {
                format!("  (cauda incompleta: {} bytes)", v.bytes_lixo_final)
            } else {
                String::new()
            }
        );
        blocos += v.blocos.len();
        ruins += r;
        leituras += n;
    }
    let wal_n = std::fs::read(dir.join("wal.log"))
        .map(|b| wal::varrer_publico(&b).len())
        .unwrap_or(0);
    println!("total: {} segmentos, {blocos} blocos, {ruins} corrompidos, {leituras} leituras + {wal_n} no WAL", segs.len());
    if ruins > 0 {
        std::process::exit(2);
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct ResultadoBench {
    leituras: u64,
    segundos: f64,
    leituras_por_s: f64,
    mb_por_s_entrada: f64,
    bytes_disco: u64,
    bytes_csv: u64,
    bytes_crus: u64,
    compressao_vs_csv: f64,
    compressao_vs_cru: f64,
    fsync: bool,
}

fn bench(total: u64, sensores: u16, fsync: bool, json: bool) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("edgelog-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ConfigGravacao {
        dir: dir.clone(),
        fsync,
        ..Default::default()
    };
    let c = Contadores::novo();
    let (gravador, _) = Gravador::abrir(cfg, c.clone())?;

    // dados gerados antes, para medir só a gravação
    let mut sim = source::Simulador::novo(ConfigSim {
        sensores,
        taxa: 100_000,
        total: Some(total),
        semente_ts: Some(1_700_000_000_000_000),
    });
    let mut rng = rand::thread_rng();
    let dados: Vec<Leitura> = (0..total)
        .map(|_| sim.proxima(&mut rng, 1_700_000_000_000_000))
        .collect();
    let bytes_csv: u64 = dados
        .iter()
        .map(|l| format!("{},{},{}\n", l.sensor_id, l.timestamp_us, l.valor).len() as u64)
        .sum();

    let (tx, rx) = crossbeam_channel::bounded::<Leitura>(262_144);
    let inicio = Instant::now();
    let t = std::thread::spawn(move || {
        for l in dados {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    gravador.rodar(rx)?;
    t.join().unwrap();
    let s = inicio.elapsed().as_secs_f64();

    let bytes_disco: u64 = listar_segmentos(&dir)?
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum();
    let bytes_crus = total * model::TAM_LEITURA as u64;
    let r = ResultadoBench {
        leituras: total,
        segundos: s,
        leituras_por_s: total as f64 / s,
        mb_por_s_entrada: bytes_crus as f64 / 1e6 / s,
        bytes_disco,
        bytes_csv,
        bytes_crus,
        compressao_vs_csv: bytes_csv as f64 / bytes_disco as f64,
        compressao_vs_cru: bytes_crus as f64 / bytes_disco as f64,
        fsync,
    };
    let _ = std::fs::remove_dir_all(&dir);
    if json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        println!("leituras        {}", r.leituras);
        println!("tempo           {:.2} s", r.segundos);
        println!(
            "vazão           {:.0} leituras/s ({:.1} MB/s de dado cru)",
            r.leituras_por_s, r.mb_por_s_entrada
        );
        println!("disco           {:.2} MB", r.bytes_disco as f64 / 1e6);
        println!(
            "compressão      {:.1}x vs CSV | {:.1}x vs binário cru",
            r.compressao_vs_csv, r.compressao_vs_cru
        );
        println!(
            "fsync           {}",
            if fsync { "ligado" } else { "desligado" }
        );
    }
    Ok(())
}
