# Plan: Edge Logger em Rust

## Arquitetura

```
 fontes (thread própria)          canal limitado            gravador (thread única)
 ┌──────────────┐                (crossbeam bounded)        ┌───────────────────────────┐
 │ sim / mqtt / │ ── Leitura ──▶ [■■■■□□□□] ── lote ──▶     │ WAL: append + fsync lote   │
 │ stdin        │   backpressure                            │ ao fechar bloco → segmento │
 └──────────────┘                                           │ zstd + delta + CRC32       │
                                                            └───────────────────────────┘
 métricas: AtomicU64 (recebidas, gravadas, descartes, bytes) → thread de status a cada N s
```

## Formato em disco

Diretório de dados:
```
data/
  wal.log                 # registros ainda não compactados
  seg-000001.edl          # segmentos imutáveis
  seg-000002.edl
```

**WAL** = sequência de registros `[len u32][crc32 u32][payload]`, payload = leituras cruas
(`u16 sensor | u64 ts_us | f64 valor` = 18 bytes). Recuperação: ler até o primeiro registro com
len impossível ou CRC inválido → truncar ali (`set_len`).

**Segmento `.edl`:**
```
header: magic "EDL1" | versão u16
bloco*: [magic_bloco u32][n_leituras u32][len_comprimido u32][crc32 u32][dados zstd]
footer: índice (offset, sensor_min..max, ts_min, ts_max por bloco) + offset do índice (u64)
```
Dados do bloco antes do zstd: colunas separadas por sensor →
- timestamps: delta-of-delta em varint zigzag
- valores: XOR com o valor anterior (estilo Gorilla) em bytes + zstd por cima

Escolha: colunar + delta + zstd dá a compressão de 5x ou mais da CA-02 sem escrever um codec Gorilla bit a bit
(mais simples de manter; o zstd pega os zeros que o XOR gera).

## Fluxo de gravação
1. O gravador recebe as leituras do canal e acumula num buffer do WAL.
2. A cada `fsync_interval` (padrão 100 ms) ou 64 KB: grava o registro no WAL e faz `fsync` (RN-02).
3. A cada `block_size` leituras (padrão 65.536): codifica o bloco, faz append no segmento atual, `fsync` do segmento e trunca o WAL.
4. Rotação de segmento por tamanho (padrão 64 MB) ou tempo (padrão 1 h): escreve o footer e fecha.
5. Na partida: recuperar o WAL, depois reabrir o último segmento sem footer, varrer os blocos válidos e reescrever o footer.

## Crates
| Crate | Uso |
|---|---|
| clap (derive) | CLI |
| crossbeam-channel | canal limitado |
| zstd | compressão |
| crc32fast | integridade |
| serde, serde_json, toml | entrada JSON, config |
| rumqttc | fonte MQTT |
| anyhow, thiserror | erros |
| rand, rand_distr | simulador |
| criterion (dev) | benchmarks dos codecs |
| tempfile, assert_cmd (dev) | testes |

## Testes
- Unitários: codec de ida e volta (proptest-like com rand), CRC detecta bit trocado, recuperação do WAL com truncamento em todo byte possível.
- Integração: `simulate | record` 1M leituras → `read --csv` bate exatamente (CA-04).
- Queda de energia (CA-03): o teste gera o processo filho `record`, mata no meio e reabre → `verify` OK.
- MQTT (CA-05): `docker run eclipse-mosquitto` + publicar N mensagens → N gravadas. Fica atrás de `--ignored` para não exigir Docker no CI padrão (no CI roda com service container no Linux).

## CI / Release
- GitHub Actions: matriz ubuntu + windows → fmt, clippy -D warnings, test.
- Release na tag `v*`: build x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu (via `cross`) → anexar os binários.

## Riscos
- Disco do notebook (SSD) limita o fsync: medir; o bench também roda com `--no-fsync` para isolar a CPU.
- Windows: `fsync` = `FlushFileBuffers`, mais lento, então documentar os números por SO.
