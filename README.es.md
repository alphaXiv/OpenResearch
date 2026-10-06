<div align="center">

<h1><img src=".github/readme-assets/openresearch.svg" alt="" width="56" align="absmiddle" /> OpenResearch</h1>

El entorno de ejecución y espacio de trabajo local para agentes de investigación.

<p><em>an <a href="https://alphaxiv.org"><img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv</a> project</em></p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest"><img src="https://img.shields.io/github/v/release/alphaXiv/OpenResearch?style=flat-square" alt="Latest release" /></a>
<a href="https://github.com/alphaXiv/OpenResearch/blob/main/LICENSE"><img src="https://img.shields.io/github/license/alphaXiv/OpenResearch?style=flat-square" alt="License" /></a>
</p>

<p><a href="README.md">English</a> · <a href="README.es.md">Español</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">简体中文</a> · <a href="README.ja.md">日本語</a></p>

<p><img src=".github/readme-assets/openresearch-screenshot.png" alt="OpenResearch desktop app showing a research agent conversation and experiment logs" width="680" /></p>

<hr />

<p>Tanto si buscas un copiloto como una herramienta de investigación autónoma, OpenResearch acelerará tu trabajo. Pon en marcha agentes de investigación capaces de revisar bibliografía, formular hipótesis, ejecutar experimentos y generar resultados de investigación.</p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-macos-dark.svg"><img src=".github/readme-assets/download-macos.svg" alt="Download OpenResearch for macOS" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-windows-dark.svg"><img src=".github/readme-assets/download-windows.svg" alt="Download OpenResearch for Windows (Beta)" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-linux-dark.svg"><img src=".github/readme-assets/download-linux.svg" alt="Download OpenResearch for Linux" width="220" height="44" /></picture></a>
</p>

<p>
<a href="https://openresearch.sh/docs"><img src=".github/readme-assets/action-documentation.svg" alt="Documentation" width="132" height="24" /></a><img src=".github/readme-assets/action-separator.svg" alt=" · " width="12" height="24" />
<a href="https://github.com/alphaXiv/OpenResearch/releases"><img src=".github/readme-assets/action-releases.svg" alt="Releases" width="78" height="24" /></a>
</p>

<p><sub>macOS 11+ · La versión beta para Windows requiere <a href="docs/windows.md">Git for Windows</a> · La app para <a href="docs/linux.md">Linux</a> requiere glibc 2.35+</sub></p>

<p><a href="https://trendshift.io/repositories/89363"><img src="https://trendshift.io/api/badge/repositories/89363" alt="GitHub Trending: #1 Repository of the Day" width="250" height="55" /></a>
<a href="https://trendshift.io/repositories/89363?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-89363" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/89363/daily?language=Rust" alt="alphaXiv/OpenResearch | Trendshift" width="250" height="55" /></a></p>

</div>

## Primeros pasos

**Recomendamos usar la aplicación de escritorio de OpenResearch.** Descárgala para tu plataforma:

- [macOS](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg)
- [Windows (Beta)](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe)
- Linux: [x86_64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage) · [ARM64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-aarch64.AppImage)

<details>
<summary>¿Prefieres usar solo la CLI? Instálala en macOS o Linux</summary>

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

`orx up` abre el panel local en `http://127.0.0.1:4791`.

En un Mac administrado, las políticas de gestión de dispositivos pueden bloquear la CLI instalada por
`install.sh` porque todavía no está firmada. Utiliza la aplicación de escritorio firmada y notarizada.
También puedes instalar `orx` para tu terminal desde
Settings → Updates → **Install the `orx` command** en la aplicación.

</details>

Crea una cuenta en [openresearch.sh](https://openresearch.sh) para recibir novedades por correo
y utilizar los recursos de cómputo administrados de OpenResearch.

## Cómo funciona

**Usa tu agente de programación favorito.** OpenResearch se integra de forma nativa con <img src=".github/readme-assets/claude.svg" alt="" width="18" height="18" align="texttop" /> Claude Code,
<img src=".github/readme-assets/codex.svg" alt="" width="18" height="18" align="texttop" /> Codex,
<img src=".github/readme-assets/opencode.svg" alt="" width="18" height="18" align="texttop" /> OpenCode,
<img src=".github/readme-assets/cursor.svg" alt="" width="18" height="18" align="texttop" /> Cursor y
<img src=".github/readme-assets/antigravity.svg" alt="" width="18" height="18" align="texttop" /> Google Antigravity.
También puedes [usar un modelo local](docs/local-models.md) con OpenCode a través de
<img src=".github/readme-assets/lmstudio.svg" alt="" width="18" height="18" align="texttop" /> LM Studio,
<img src=".github/readme-assets/omlx.svg" alt="" width="18" height="18" align="texttop" /> oMLX,
<img src=".github/readme-assets/ollama.svg" alt="" width="18" height="18" align="texttop" /> Ollama o un endpoint personalizado.

**Fundamenta tus hipótesis en la bibliografía más reciente.** OpenResearch se integra con <img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv, <img src=".github/readme-assets/biorxiv.svg" alt="" width="18" height="18" align="absmiddle" /> bioRxiv y <img src=".github/readme-assets/pubmed.svg" alt="" width="18" height="18" align="absmiddle" /> PubMed para que los agentes encuentren artículos relevantes, formulen hipótesis informadas y conecten sus experimentos con investigaciones existentes.

**Elige dónde ejecutar tus experimentos.** Ejecútalos en local o utiliza SSH,
Slurm,
<img src=".github/readme-assets/kubernetes.svg" alt="" width="18" height="18" align="texttop" /> Kubernetes,
<img src=".github/readme-assets/modal.svg" alt="" width="18" height="18" align="texttop" /> Modal,
<img src=".github/readme-assets/thinking-machines.svg" alt="" width="18" height="18" align="texttop" /> Tinker,
<img src=".github/readme-assets/ray.svg" alt="" width="18" height="18" align="texttop" /> Ray,
<img src=".github/readme-assets/huggingface.svg" alt="" width="18" height="18" align="texttop" /> Hugging Face Jobs y
los recursos de cómputo administrados de OpenResearch.

**Dale a tus agentes una memoria de cada experimento.** OpenResearch registra cada experimento en una base de datos SQL local y conserva sus logs, código
y artefactos en tu equipo para que los agentes puedan
aprender de los resultados anteriores y decidir qué probar después. La aplicación de escritorio
te permite ver fácilmente lo que han hecho tus agentes y examinar la evidencia de cada resultado.

**Tu investigación te pertenece.** Ejecuta todo en local. No recopilamos tu código ni las trazas de tus agentes.
Tus proyectos, conversaciones, experimentos, registros y artefactos permanecen en tu equipo,
bajo tu control.

## Analíticas de uso

Las versiones oficiales envían eventos generales de uso asociados a un identificador de instalación
aleatorio; puedes desactivarlos. No incluyen código, prompts, contenido ni rutas de archivos,
nombres de repositorios, tokens, correos electrónicos ni identificadores de proyectos o experimentos.

Puedes desactivar las analíticas de uso en Settings de la aplicación de escritorio o con `orx telemetry off`.
