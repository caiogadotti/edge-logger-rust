//! Bloco comprimido: leituras agrupadas por sensor, timestamps em delta-of-delta (varint zigzag),
//! valores em XOR com o anterior; zstd por cima e CRC32 do resultado.

use crate::model::Leitura;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

const NIVEL_ZSTD: i32 = 3;

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(b: &[u8], pos: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let byte = *b.get(*pos).context("varint truncado")?;
        *pos += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
        if shift > 63 {
            bail!("varint inválido");
        }
    }
}

/// Codifica as leituras (qualquer ordem) num bloco. Ordem original dentro de cada sensor é preservada.
pub fn codificar(leituras: &[Leitura]) -> Result<Vec<u8>> {
    let mut por_sensor: BTreeMap<u16, Vec<&Leitura>> = BTreeMap::new();
    for l in leituras {
        por_sensor.entry(l.sensor_id).or_default().push(l);
    }

    let mut cru = Vec::with_capacity(leituras.len() * 6);
    put_varint(&mut cru, por_sensor.len() as u64);
    for (sensor, ls) in &por_sensor {
        put_varint(&mut cru, *sensor as u64);
        put_varint(&mut cru, ls.len() as u64);

        let (mut ts_ant, mut delta_ant) = (0i64, 0i64);
        for l in ls {
            let ts = l.timestamp_us as i64;
            let delta = ts.wrapping_sub(ts_ant);
            put_varint(&mut cru, zigzag(delta.wrapping_sub(delta_ant)));
            ts_ant = ts;
            delta_ant = delta;
        }

        let mut bits_ant = 0u64;
        for l in ls {
            let bits = l.valor.to_bits();
            cru.extend_from_slice(&(bits ^ bits_ant).to_le_bytes());
            bits_ant = bits;
        }
    }

    Ok(zstd::encode_all(&cru[..], NIVEL_ZSTD)?)
}

pub fn decodificar(comprimido: &[u8]) -> Result<Vec<Leitura>> {
    let cru = zstd::decode_all(comprimido).context("zstd inválido")?;
    let mut pos = 0;
    let n_sensores = get_varint(&cru, &mut pos)?;
    let mut out = Vec::new();

    for _ in 0..n_sensores {
        let sensor = get_varint(&cru, &mut pos)? as u16;
        let n = get_varint(&cru, &mut pos)? as usize;

        let mut tss = Vec::with_capacity(n);
        let (mut ts, mut delta) = (0i64, 0i64);
        for _ in 0..n {
            delta = delta.wrapping_add(unzigzag(get_varint(&cru, &mut pos)?));
            ts = ts.wrapping_add(delta);
            tss.push(ts as u64);
        }

        let mut bits = 0u64;
        for ts in tss {
            let fim = pos + 8;
            let x = u64::from_le_bytes(cru.get(pos..fim).context("valores truncados")?.try_into()?);
            pos = fim;
            bits ^= x;
            out.push(Leitura {
                sensor_id: sensor,
                timestamp_us: ts,
                valor: f64::from_bits(bits),
            });
        }
    }
    Ok(out)
}

pub fn crc(dados: &[u8]) -> u32 {
    crc32fast::hash(dados)
}

#[cfg(test)]
mod testes {
    use super::*;
    use rand::Rng;

    fn amostra(n: usize) -> Vec<Leitura> {
        let mut rng = rand::thread_rng();
        (0..n)
            .map(|i| Leitura {
                sensor_id: (i % 4) as u16,
                timestamp_us: 1_000_000 + (i as u64) * 10 + rng.gen_range(0..3),
                valor: (i as f64 * 0.01).sin() * 100.0 + rng.gen::<f64>(),
            })
            .collect()
    }

    fn ordenar(mut v: Vec<Leitura>) -> Vec<Leitura> {
        v.sort_by_key(|l| (l.sensor_id, l.timestamp_us));
        v
    }

    #[test]
    fn ida_e_volta() {
        let ls = amostra(10_000);
        let dec = decodificar(&codificar(&ls).unwrap()).unwrap();
        assert_eq!(ordenar(dec), ordenar(ls));
    }

    #[test]
    fn timestamp_fora_de_ordem() {
        let ls = vec![
            Leitura {
                sensor_id: 1,
                timestamp_us: 500,
                valor: 1.0,
            },
            Leitura {
                sensor_id: 1,
                timestamp_us: 100,
                valor: 2.0,
            },
            Leitura {
                sensor_id: 1,
                timestamp_us: 900,
                valor: f64::NAN,
            },
        ];
        let dec = decodificar(&codificar(&ls).unwrap()).unwrap();
        assert_eq!(dec[1].timestamp_us, 100);
        assert!(dec[2].valor.is_nan());
    }

    #[test]
    fn crc_detecta_bit_trocado() {
        let mut b = codificar(&amostra(100)).unwrap();
        let antes = crc(&b);
        b[5] ^= 0b0000_0100;
        assert_ne!(crc(&b), antes);
    }

    #[test]
    fn comprime_melhor_que_csv() {
        let ls = amostra(65_536);
        let csv: usize = ls
            .iter()
            .map(|l| format!("{},{},{}\n", l.sensor_id, l.timestamp_us, l.valor).len())
            .sum();
        let bloco = codificar(&ls).unwrap().len();
        assert!(csv / bloco >= 2, "csv={csv} bloco={bloco}");
    }
}
