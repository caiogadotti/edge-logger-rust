//! Segmento `.edl`: header + sequência de blocos autodescritos.
//! Cada bloco carrega ts_min/ts_max no cabeçalho, então filtrar por tempo não precisa de índice
//! no fim do arquivo, e um segmento interrompido pela queda de energia continua legível sem reparo.

use crate::codec;
use crate::model::Leitura;
use anyhow::{bail, Result};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

const MAGIC_ARQ: &[u8; 4] = b"EDL1";
const VERSAO: u16 = 1;
const MAGIC_BLOCO: u32 = u32::from_le_bytes(*b"EDLB");
const CAB_BLOCO: usize = 32;

pub struct EscritorSegmento {
    arquivo: File,

    pub bytes: u64,
}

impl EscritorSegmento {
    pub fn criar(caminho: &Path) -> Result<Self> {
        let existe = caminho.exists() && std::fs::metadata(caminho)?.len() > 0;
        let mut arquivo = OpenOptions::new().create(true).append(true).open(caminho)?;
        if !existe {
            arquivo.write_all(MAGIC_ARQ)?;
            arquivo.write_all(&VERSAO.to_le_bytes())?;
        }
        let bytes = arquivo.metadata()?.len();
        Ok(Self { arquivo, bytes })
    }

    /// Codifica e grava um bloco com fsync. Devolve os bytes escritos.
    pub fn gravar_bloco(&mut self, leituras: &[Leitura], fsync: bool) -> Result<usize> {
        if leituras.is_empty() {
            return Ok(0);
        }
        let dados = codec::codificar(leituras)?;
        let ts_min = leituras.iter().map(|l| l.timestamp_us).min().unwrap();
        let ts_max = leituras.iter().map(|l| l.timestamp_us).max().unwrap();

        let mut buf = Vec::with_capacity(CAB_BLOCO + dados.len());
        buf.extend_from_slice(&MAGIC_BLOCO.to_le_bytes());
        buf.extend_from_slice(&(leituras.len() as u32).to_le_bytes());
        buf.extend_from_slice(&ts_min.to_le_bytes());
        buf.extend_from_slice(&ts_max.to_le_bytes());
        buf.extend_from_slice(&(dados.len() as u32).to_le_bytes());
        buf.extend_from_slice(&codec::crc(&dados).to_le_bytes());
        buf.extend_from_slice(&dados);
        self.arquivo.write_all(&buf)?;
        if fsync {
            self.arquivo.sync_data()?;
        }
        self.bytes += buf.len() as u64;
        Ok(buf.len())
    }
}

#[derive(Debug, Clone)]
pub struct InfoBloco {
    pub n: u32,
    pub ts_min: u64,
    pub ts_max: u64,
    pub ok: bool,
}

pub struct Varredura {
    pub blocos: Vec<InfoBloco>,
    pub leituras: Vec<Leitura>,
    pub bytes_lixo_final: u64,
}

/// Lê o segmento inteiro. Bloco com CRC ruim é marcado e pulado (RN-05); cauda incompleta é ignorada.
pub fn ler_segmento(caminho: &Path, filtro: &dyn Fn(&InfoBloco) -> bool) -> Result<Varredura> {
    let mut b = Vec::new();
    BufReader::new(File::open(caminho)?).read_to_end(&mut b)?;
    if b.len() < 6 || &b[..4] != MAGIC_ARQ {
        bail!("{} não é um segmento EDL1", caminho.display());
    }

    let mut pos = 6usize;
    let mut blocos = Vec::new();
    let mut leituras = Vec::new();
    while pos + CAB_BLOCO <= b.len() {
        let u32_em = |i: usize| u32::from_le_bytes(b[pos + i..pos + i + 4].try_into().unwrap());
        let u64_em = |i: usize| u64::from_le_bytes(b[pos + i..pos + i + 8].try_into().unwrap());
        if u32_em(0) != MAGIC_BLOCO {
            match ressincronizar(&b, pos + 1) {
                Some(p) => {
                    blocos.push(InfoBloco {
                        n: 0,
                        ts_min: 0,
                        ts_max: 0,
                        ok: false,
                    });
                    pos = p;
                    continue;
                }
                None => break,
            }
        }
        let (n, ts_min, ts_max, len, soma) = (
            u32_em(4),
            u64_em(8),
            u64_em(16),
            u32_em(24) as usize,
            u32_em(28),
        );
        let ini = pos + CAB_BLOCO;
        if ini + len > b.len() {
            break;
        }
        let dados = &b[ini..ini + len];
        let mut info = InfoBloco {
            n,
            ts_min,
            ts_max,
            ok: codec::crc(dados) == soma,
        };
        if info.ok && filtro(&info) {
            match codec::decodificar(dados) {
                Ok(ls) => leituras.extend(ls),
                Err(_) => info.ok = false,
            }
        }
        blocos.push(info);
        pos = ini + len;
    }
    Ok(Varredura {
        blocos,
        leituras,
        bytes_lixo_final: (b.len() - pos) as u64,
    })
}

fn ressincronizar(b: &[u8], desde: usize) -> Option<usize> {
    let alvo = MAGIC_BLOCO.to_le_bytes();
    b.get(desde..)?
        .windows(4)
        .position(|w| w == alvo)
        .map(|p| desde + p)
}

pub fn listar_segmentos(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "edl"))
        .collect();
    v.sort();
    Ok(v)
}

pub fn nome_segmento(dir: &Path, n: u32) -> PathBuf {
    dir.join(format!("seg-{n:06}.edl"))
}

#[cfg(test)]
mod testes {
    use super::*;

    fn lote(ini: u64, n: u64) -> Vec<Leitura> {
        (ini..ini + n)
            .map(|i| Leitura {
                sensor_id: (i % 2) as u16,
                timestamp_us: i,
                valor: i as f64 / 3.0,
            })
            .collect()
    }

    #[test]
    fn grava_le_e_filtra() {
        let dir = tempfile::tempdir().unwrap();
        let p = nome_segmento(dir.path(), 1);
        let mut w = EscritorSegmento::criar(&p).unwrap();
        w.gravar_bloco(&lote(0, 1000), true).unwrap();
        w.gravar_bloco(&lote(1000, 1000), true).unwrap();
        let tudo = ler_segmento(&p, &|_| true).unwrap();
        assert_eq!(tudo.leituras.len(), 2000);
        let so_segundo = ler_segmento(&p, &|b| b.ts_min >= 1000).unwrap();
        assert_eq!(so_segundo.leituras.len(), 1000);
    }

    #[test]
    fn bloco_corrompido_e_pulado() {
        let dir = tempfile::tempdir().unwrap();
        let p = nome_segmento(dir.path(), 1);
        let mut w = EscritorSegmento::criar(&p).unwrap();
        for i in 0..3 {
            w.gravar_bloco(&lote(i * 500, 500), true).unwrap();
        }
        let mut b = std::fs::read(&p).unwrap();
        let meio = 6 + CAB_BLOCO + 10;
        b[meio] ^= 0xff;
        std::fs::write(&p, &b).unwrap();
        let v = ler_segmento(&p, &|_| true).unwrap();
        assert_eq!(v.blocos.iter().filter(|b| !b.ok).count(), 1);
        assert_eq!(v.leituras.len(), 1000);
    }

    #[test]
    fn cauda_incompleta_e_ignorada() {
        let dir = tempfile::tempdir().unwrap();
        let p = nome_segmento(dir.path(), 1);
        let mut w = EscritorSegmento::criar(&p).unwrap();
        w.gravar_bloco(&lote(0, 500), true).unwrap();
        w.gravar_bloco(&lote(500, 500), true).unwrap();
        let b = std::fs::read(&p).unwrap();
        std::fs::write(&p, &b[..b.len() - 7]).unwrap();
        let v = ler_segmento(&p, &|_| true).unwrap();
        assert_eq!(v.leituras.len(), 500);
        assert!(v.bytes_lixo_final > 0);
    }
}
