# Spec: Edge Logger de Telemetria em Rust

## Problema / Motivação
Em chão de fábrica, sensores (corrente, temperatura, vibração) geram milhares de leituras por segundo.
O PC industrial ou o Raspberry Pi que fica ao lado da máquina precisa gravar tudo **sem perder dado**:
nem se a rede cair, nem se a energia acabar no meio da escrita. Loggers feitos em Python ou Node
costumam travar ou perder amostras nessa taxa. O projeto mostra um logger de borda em Rust
que aguenta alta vazão, sobrevive a queda de energia e ocupa pouco disco.
Também serve de portfólio: é a primeira linguagem de sistemas no GitHub e se conecta ao projeto
da catenária, que já publica telemetria via MQTT.

## Escopo (O QUE FAZ)
- Binário de linha de comando `edgelog` com subcomandos:
  - `edgelog record`: recebe leituras e grava em disco.
  - `edgelog simulate`: gera sensores sintéticos (senoide + ruído + picos de falha) numa taxa configurável.
  - `edgelog read`: lê os arquivos gravados e exporta para CSV ou JSON, com filtro por sensor e intervalo de tempo.
  - `edgelog bench`: mede a vazão (leituras/s) e a taxa de compressão.
  - `edgelog verify`: confere a integridade dos arquivos (checksum por bloco) e relata blocos corrompidos.
- Três fontes de entrada: **simulador interno**, **MQTT** (assina um tópico, payload JSON) e **stdin** (JSON por linha).
- Formato de arquivo próprio em segmentos:
  - um WAL (write-ahead log) com append e `fsync` em lote;
  - compactação em blocos, com delta encoding de timestamp e valor e compressão zstd;
  - CRC32 em cada bloco;
  - rotação de segmento por tamanho ou tempo.
- Recuperação automática ao iniciar: um WAL incompleto (energia caiu) é truncado no último registro válido e o restante é aproveitado.
- Pipeline concorrente: a recepção e a gravação rodam em threads separadas, ligadas por um canal com limite de tamanho (backpressure em vez de estourar a memória).
- Configuração por arquivo TOML e flags de CLI.
- Métricas no terminal a cada N segundos: leituras/s, bytes/s, fila e descartes.

## Fora do Escopo (O QUE NÃO FAZ)
- Interface gráfica ou dashboard web: o foco é o motor; o CSV exportado alimenta outros projetos.
- Banco de dados (Postgres/InfluxDB): o formato em arquivo é a proposta.
- Firmware para microcontrolador: o logger roda em Windows/Linux, inclusive ARM (Raspberry Pi).
- Autenticação ou TLS no MQTT, nesta versão.
- Replicação ou envio para a nuvem.

## Regras de Negócio
- RN-01: uma leitura é `{ sensor_id: u16, timestamp_us: u64, valor: f64 }`.
- RN-02: nenhuma leitura aceita pode ser perdida depois do `fsync`; no máximo o lote em andamento (configurável, padrão 100 ms) se perde numa queda de energia.
- RN-03: fila cheia → a fonte espera (backpressure). Só o modo `--drop-on-full` descarta, e conta os descartes.
- RN-04: timestamp fora de ordem dentro do mesmo sensor é aceito e marcado; não quebra a compressão.
- RN-05: arquivo corrompido nunca derruba o `read`; os blocos inválidos são pulados e relatados.

## Critérios de Aceite
- [ ] CA-01: `edgelog simulate --rate 500000 | edgelog record` sustenta ≥ 500 mil leituras/s por 60 s num notebook comum, sem crescer a memória sem limite.
- [ ] CA-02: compressão ≥ 5x em dados de sensor realistas (senoide + ruído) em comparação com CSV.
- [ ] CA-03: teste de "queda de energia": matar o processo (`kill -9`) durante a gravação, religar → `verify` mostra 0 blocos corrompidos e perda ≤ 1 lote.
- [ ] CA-04: `read --csv` devolve exatamente as leituras gravadas (teste de ida e volta com 1 milhão de leituras).
- [ ] CA-05: entrada MQTT funcionando contra um broker local (mosquitto em Docker), testada em integração.
- [ ] CA-06: `cargo test` e `cargo clippy -- -D warnings` limpos; CI no GitHub Actions para Linux e Windows.
- [ ] CA-07: binários para Windows x64, Linux x64 e Linux ARM64 publicados nas Releases.
- [ ] CA-08: README em PT (principal) e EN, com o diagrama da arquitetura, o gráfico do benchmark e o GIF do terminal.

## Arquivos que serão afetados
Projeto novo em `C:\Users\programacao\Desktop\PESSOAL\Projetos Pessoais\edge-logger-rust\`:
- `src/main.rs`: CLI (clap)
- `src/source/{sim,mqtt,stdin}.rs`: fontes de entrada
- `src/wal.rs`: write-ahead log + recuperação
- `src/segment.rs`: blocos comprimidos, delta encoding e CRC
- `src/pipeline.rs`: threads e canal limitado
- `src/bench.rs`, `src/export.rs`
- `tests/`: ida e volta, queda de energia, arquivo corrompido
- `.github/workflows/ci.yml`, `README.md`, `README.en.md`

## Dependências / Integrações
- Toolchain Rust (rustup) + Visual Studio Build Tools (linker MSVC), **a instalar**.
- Crates: `clap`, `serde`/`serde_json`, `toml`, `zstd`, `crc32fast`, `crossbeam-channel`, `rumqttc` (MQTT), `criterion` (benchmark).
- Docker (já instalado) para o broker mosquitto nos testes de integração.
- Ligação com o portfólio: pode receber a telemetria do projeto `monitoramento-catenaria`.
