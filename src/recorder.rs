//! Gravador: thread única que consome o canal, mantém o WAL e fecha blocos em segmentos.

use crate::model::Leitura;
use crate::segment::{ler_segmento, listar_segmentos, nome_segmento, EscritorSegmento};
use crate::source::Contadores;
use crate::wal::Wal;
use anyhow::Result;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
pub struct ConfigGravacao {
    pub dir: PathBuf,
    pub bloco: usize,
    pub fsync_ms: u64,
    pub fsync: bool,
    pub segmento_mb: u64,
    pub segmento_seg: u64,
}

impl Default for ConfigGravacao {
    fn default() -> Self {
        Self {
            dir: "data".into(),
            bloco: 65_536,
            fsync_ms: 100,
            fsync: true,
            segmento_mb: 64,
            segmento_seg: 3600,
        }
    }
}

pub struct Gravador {
    cfg: ConfigGravacao,
    wal: Wal,
    seg: EscritorSegmento,
    num_seg: u32,
    aberto_em: Instant,
    buffer: Vec<Leitura>,
    c: Arc<Contadores>,
}

pub struct Partida {
    pub recuperadas_wal: usize,
    pub bytes_truncados: u64,
}

impl Gravador {
    pub fn abrir(cfg: ConfigGravacao, c: Arc<Contadores>) -> Result<(Self, Partida)> {
        std::fs::create_dir_all(&cfg.dir)?;
        let segs = listar_segmentos(&cfg.dir)?;
        let num_seg = match segs.last() {
            Some(p) => {
                aparar_cauda(p)?;
                numero(p).unwrap_or(1)
            }
            None => 1,
        };
        let seg = EscritorSegmento::criar(&nome_segmento(&cfg.dir, num_seg))?;
        let (wal, rec) = Wal::abrir(&cfg.dir.join("wal.log"))?;
        let mut g = Self {
            cfg,
            wal,
            seg,
            num_seg,
            aberto_em: Instant::now(),
            buffer: Vec::new(),
            c,
        };
        let partida = Partida {
            recuperadas_wal: rec.leituras.len(),
            bytes_truncados: rec.bytes_truncados,
        };
        // o que estava no WAL vira bloco já na partida, assim o WAL volta a zero
        if !rec.leituras.is_empty() {
            g.buffer = rec.leituras;
            g.fechar_bloco()?;
        }
        Ok((g, partida))
    }

    fn fechar_bloco(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.wal.sincronizar(self.cfg.fsync)?;
        let n = self.seg.gravar_bloco(&self.buffer, self.cfg.fsync)?;
        self.c.bytes_disco.fetch_add(n as u64, Ordering::Relaxed);
        self.c
            .gravadas
            .fetch_add(self.buffer.len() as u64, Ordering::Relaxed);
        self.buffer.clear();
        self.wal.zerar()?;

        let estourou_tamanho = self.seg.bytes >= self.cfg.segmento_mb * 1024 * 1024;
        let estourou_tempo = self.aberto_em.elapsed() >= Duration::from_secs(self.cfg.segmento_seg);
        if estourou_tamanho || estourou_tempo {
            self.num_seg += 1;
            self.seg = EscritorSegmento::criar(&nome_segmento(&self.cfg.dir, self.num_seg))?;
            self.aberto_em = Instant::now();
        }
        Ok(())
    }

    pub fn rodar(mut self, rx: Receiver<Leitura>) -> Result<()> {
        let intervalo = Duration::from_millis(self.cfg.fsync_ms.max(1));
        let mut ultimo_sync = Instant::now();
        loop {
            match rx.recv_timeout(intervalo) {
                Ok(l) => {
                    self.wal.adicionar(&l);
                    self.buffer.push(l);
                    // esvazia o que já está na fila sem voltar ao timeout
                    for l in rx.try_iter().take(self.cfg.bloco) {
                        self.wal.adicionar(&l);
                        self.buffer.push(l);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.buffer.len() >= self.cfg.bloco {
                self.fechar_bloco()?;
                ultimo_sync = Instant::now();
            } else if ultimo_sync.elapsed() >= intervalo || self.wal.pendente_bytes() >= 64 * 1024 {
                self.wal.sincronizar(self.cfg.fsync)?;
                ultimo_sync = Instant::now();
            }
        }
        self.fechar_bloco()
    }
}

fn numero(p: &Path) -> Option<u32> {
    p.file_stem()?.to_str()?.strip_prefix("seg-")?.parse().ok()
}

/// Remove bytes de um bloco que a queda de energia deixou pela metade, antes de voltar a anexar.
fn aparar_cauda(p: &Path) -> Result<()> {
    let v = ler_segmento(p, &|_| false)?;
    if v.bytes_lixo_final > 0 {
        let f = std::fs::OpenOptions::new().write(true).open(p)?;
        f.set_len(f.metadata()?.len() - v.bytes_lixo_final)?;
        f.sync_all()?;
    }
    Ok(())
}
