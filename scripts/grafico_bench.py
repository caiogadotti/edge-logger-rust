"""Gera docs/bench.png (e bench-en.png) a partir dos JSON de `edgelog bench --json`."""
import json
from pathlib import Path

import matplotlib.pyplot as plt

DOCS = Path(__file__).resolve().parent.parent / "docs"
COR = "#C2542D"
CINZA = "#9A9A9A"

TEXTOS = {
    "pt": {
        "titulo_vazao": "Vazão de gravação",
        "y_vazao": "milhões de leituras/s",
        "rot_vazao": ["meta\n(spec)", "com fsync\n(sem perda)", "sem fsync\n(só CPU)"],
        "titulo_disco": "Espaço em disco: 5 milhões de leituras",
        "y_disco": "MB",
        "rot_disco": ["CSV", "binário cru", "edgelog"],
        "rodape": "i5-12450HX · SSD NVMe · Windows 11 · 8 sensores",
        "arquivo": "bench.png",
    },
    "en": {
        "titulo_vazao": "Write throughput",
        "y_vazao": "million readings/s",
        "rot_vazao": ["target\n(spec)", "with fsync\n(no loss)", "no fsync\n(CPU only)"],
        "titulo_disco": "Disk usage: 5 million readings",
        "y_disco": "MB",
        "rot_disco": ["CSV", "raw binary", "edgelog"],
        "rodape": "i5-12450HX · NVMe SSD · Windows 11 · 8 sensors",
        "arquivo": "bench-en.png",
    },
}


def gerar(lang: str, com: dict, sem: dict):
    t = TEXTOS[lang]
    fig, (a, b) = plt.subplots(1, 2, figsize=(11, 4.2), dpi=150)
    for ax in (a, b):
        ax.spines[["top", "right"]].set_visible(False)

    vaz = [0.5, com["leituras_por_s"] / 1e6, sem["leituras_por_s"] / 1e6]
    barras = a.bar(t["rot_vazao"], vaz, color=[CINZA, COR, "#E3A587"])
    a.bar_label(barras, labels=[f"{v:.1f}" for v in vaz], padding=3)
    a.set_title(t["titulo_vazao"], loc="left", fontweight="bold")
    a.set_ylabel(t["y_vazao"])

    disco = [com["bytes_csv"] / 1e6, com["bytes_crus"] / 1e6, com["bytes_disco"] / 1e6]
    barras = b.bar(t["rot_disco"], disco, color=[CINZA, CINZA, COR])
    b.bar_label(barras, labels=[f"{v:.0f}" if v >= 20 else f"{v:.1f}" for v in disco], padding=3)
    b.set_title(t["titulo_disco"], loc="left", fontweight="bold")
    b.set_ylabel(t["y_disco"])
    b.annotate(f"{com['compressao_vs_csv']:.1f}x", xy=(2, disco[2] + disco[0] * 0.08), xytext=(2, disco[0] * 0.5),
               ha="center", fontsize=13, fontweight="bold", color=COR,
               arrowprops=dict(arrowstyle="->", color=COR))

    fig.text(0.99, 0.01, t["rodape"], ha="right", fontsize=8, color=CINZA)
    fig.tight_layout()
    fig.savefig(DOCS / t["arquivo"], bbox_inches="tight")
    plt.close(fig)


if __name__ == "__main__":
    com = json.loads((DOCS / "bench-fsync.json").read_text())
    sem = json.loads((DOCS / "bench-sem-fsync.json").read_text())
    for lang in TEXTOS:
        gerar(lang, com, sem)
    print("ok")
