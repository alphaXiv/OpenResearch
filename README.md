<div align="center">

<h1><img src=".github/readme-assets/openresearch.svg" alt="" width="56" align="absmiddle" /> OpenResearch</h1>

The local-first harness & workspace for research agents.

<p><em>an <a href="https://alphaxiv.org"><img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv</a> project</em></p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest"><img src="https://img.shields.io/github/v/release/alphaXiv/OpenResearch?style=flat-square" alt="Latest release" /></a>
<a href="https://github.com/alphaXiv/OpenResearch/blob/main/LICENSE"><img src="https://img.shields.io/github/license/alphaXiv/OpenResearch?style=flat-square" alt="License" /></a>
</p>

<p><a href="README.md">English</a> · <a href="README.es.md">Español</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">简体中文</a> · <a href="README.ja.md">日本語</a></p>

<p><img src=".github/readme-assets/openresearch-screenshot.png" alt="OpenResearch desktop app showing a research agent conversation and experiment logs" width="680" /></p>

<hr />

<p>Whether you’re looking for a copilot or an autoresearch tool, OpenResearch will accelerate your work. Launch research agents that can review literature, develop hypotheses, run experiments, and produce research artifacts.</p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-macos-dark.svg"><img src=".github/readme-assets/download-macos.svg" alt="Download OpenResearch for macOS" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-windows-dark.svg"><img src=".github/readme-assets/download-windows.svg" alt="Download OpenResearch for Windows (Beta)" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-linux-dark.svg"><img src=".github/readme-assets/download-linux.svg" alt="Download OpenResearch for Linux" width="220" height="44" /></picture></a>
</p>

<p>
<a href="https://openresearch.sh/docs"><img src=".github/readme-assets/action-documentation.svg" alt="Documentation" width="132" height="24" /></a><img src=".github/readme-assets/action-separator.svg" alt=" · " width="12" height="24" />
<a href="https://github.com/alphaXiv/OpenResearch/releases"><img src=".github/readme-assets/action-releases.svg" alt="Releases" width="78" height="24" /></a>
</p>

<p><sub>macOS 11+ · Windows beta requires <a href="docs/windows.md">Git for Windows</a> · <a href="docs/linux.md">Linux</a> app needs glibc 2.35+</sub></p>

<p><a href="https://trendshift.io/repositories/89363"><img src="https://trendshift.io/api/badge/repositories/89363" alt="GitHub Trending: #1 Repository of the Day" width="250" height="55" /></a>
<a href="https://trendshift.io/repositories/89363?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-89363" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/89363/daily?language=Rust" alt="alphaXiv/OpenResearch | Trendshift" width="250" height="55" /></a></p>

</div>

## Get started

**We recommend using the OpenResearch desktop app.** Download it for your platform:

- [macOS](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg)
- [Windows (Beta)](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe)
- Linux: [x86_64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage) · [ARM64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-aarch64.AppImage)

<details>
<summary>Prefer the standalone CLI? Install on macOS or Linux</summary>

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

`orx up` opens the local dashboard at `http://127.0.0.1:4791`.

On a managed Mac, device-management policies may block the CLI installed by
`install.sh` because it is not yet signed. Use the signed and notarized desktop
app instead. You can also install `orx` for your terminal from the app's
Settings → Updates → **Install the `orx` command**.

</details>

Create an account at [openresearch.sh](https://openresearch.sh) to receive email
updates and use managed OpenResearch compute.

## How it works

**Use your favorite coding agent.** OpenResearch works natively with <img src=".github/readme-assets/claude.svg" alt="" width="18" height="18" align="texttop" /> Claude Code,
<img src=".github/readme-assets/codex.svg" alt="" width="18" height="18" align="texttop" /> Codex,
<img src=".github/readme-assets/opencode.svg" alt="" width="18" height="18" align="texttop" /> OpenCode,
<img src=".github/readme-assets/cursor.svg" alt="" width="18" height="18" align="texttop" /> Cursor, and
<img src=".github/readme-assets/antigravity.svg" alt="" width="18" height="18" align="texttop" /> Google Antigravity.
You can also [use a local model](docs/local-models.md) with OpenCode through
<img src=".github/readme-assets/lmstudio.svg" alt="" width="18" height="18" align="texttop" /> LM Studio,
<img src=".github/readme-assets/omlx.svg" alt="" width="18" height="18" align="texttop" /> oMLX,
<img src=".github/readme-assets/ollama.svg" alt="" width="18" height="18" align="texttop" /> Ollama, or a custom endpoint.

**Ground research hypotheses in the latest literature.** OpenResearch integrates with <img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv, <img src=".github/readme-assets/biorxiv.svg" alt="" width="18" height="18" align="absmiddle" /> bioRxiv, and <img src=".github/readme-assets/pubmed.svg" alt="" width="18" height="18" align="absmiddle" /> PubMed so agents can find relevant papers, develop informed hypotheses, and connect their experiments to existing research.

**Choose your compute.** Run experiments locally or use SSH,
Slurm,
<img src=".github/readme-assets/kubernetes.svg" alt="" width="18" height="18" align="texttop" /> Kubernetes,
<img src=".github/readme-assets/modal.svg" alt="" width="18" height="18" align="texttop" /> Modal,
<img src=".github/readme-assets/thinking-machines.svg" alt="" width="18" height="18" align="texttop" /> Tinker,
<img src=".github/readme-assets/ray.svg" alt="" width="18" height="18" align="texttop" /> Ray,
<img src=".github/readme-assets/huggingface.svg" alt="" width="18" height="18" align="texttop" /> Hugging Face Jobs, and
managed OpenResearch compute.

**Give agents a memory of every experiment.** OpenResearch records every experiment in a local SQL database and keeps its logs, code,
and artifacts on your machine so that agents can
learn from past results and decide what to try next. The desktop app makes it
easy to see what your agents have done and inspect the evidence behind each result.

**Keep and own your research.** Run entirely locally. We don't collect your code or agent traces. Your projects,
conversations, experiments, logs, and artifacts stay on your machine, under your
control.

## Usage analytics

Official release builds send opt-out, coarse usage events tied to a random
installation ID. They do not include code, prompts, file contents or paths,
repository names, tokens, emails, or project and experiment identifiers.

You can turn usage analytics off in the desktop app's Settings or with `orx telemetry off`.
