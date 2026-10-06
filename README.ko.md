<div align="center">

<h1><img src=".github/readme-assets/openresearch.svg" alt="" width="56" align="absmiddle" /> OpenResearch</h1>

연구 에이전트를 위한 로컬 중심 하네스 및 워크스페이스.

<p><em>an <a href="https://alphaxiv.org"><img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv</a> project</em></p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest"><img src="https://img.shields.io/github/v/release/alphaXiv/OpenResearch?style=flat-square" alt="Latest release" /></a>
<a href="https://github.com/alphaXiv/OpenResearch/blob/main/LICENSE"><img src="https://img.shields.io/github/license/alphaXiv/OpenResearch?style=flat-square" alt="License" /></a>
</p>

<p><a href="README.md">English</a> · <a href="README.es.md">Español</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">简体中文</a> · <a href="README.ja.md">日本語</a></p>

<p><img src=".github/readme-assets/openresearch-screenshot.png" alt="OpenResearch desktop app showing a research agent conversation and experiment logs" width="680" /></p>

<hr />

<p>코파일럿이 필요하든 자율 연구 도구가 필요하든, OpenResearch는 작업을 가속합니다. 문헌을 검토하고, 가설을 세우고, 실험을 실행하고, 연구 산출물을 만드는 연구 에이전트를 시작하세요.</p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-macos-dark.svg"><img src=".github/readme-assets/download-macos.svg" alt="Download OpenResearch for macOS" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-windows-dark.svg"><img src=".github/readme-assets/download-windows.svg" alt="Download OpenResearch for Windows (베타)" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-linux-dark.svg"><img src=".github/readme-assets/download-linux.svg" alt="Download OpenResearch for Linux" width="220" height="44" /></picture></a>
</p>

<p>
<a href="https://openresearch.sh/docs"><img src=".github/readme-assets/action-documentation.svg" alt="Documentation" width="132" height="24" /></a><img src=".github/readme-assets/action-separator.svg" alt=" · " width="12" height="24" />
<a href="https://github.com/alphaXiv/OpenResearch/releases"><img src=".github/readme-assets/action-releases.svg" alt="Releases" width="78" height="24" /></a>
</p>

<p><sub>macOS 11+ · Windows 베타는 다음이 필요합니다: <a href="docs/windows.md">Git for Windows</a> · <a href="docs/linux.md">Linux</a> 앱은 glibc 2.35+가 필요합니다</sub></p>

<p><a href="https://trendshift.io/repositories/89363"><img src="https://trendshift.io/api/badge/repositories/89363" alt="GitHub Trending: #1 Repository of the Day" width="250" height="55" /></a>
<a href="https://trendshift.io/repositories/89363?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-89363" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/89363/daily?language=Rust" alt="alphaXiv/OpenResearch | Trendshift" width="250" height="55" /></a></p>

</div>

## 시작하기

**OpenResearch 데스크톱 앱 사용을 권장합니다.** 사용 중인 플랫폼에 맞게 다운로드하세요:

- [macOS](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg)
- [Windows (베타)](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe)
- Linux: [x86_64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage) · [ARM64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-aarch64.AppImage)

<details>
<summary>독립 실행형 CLI를 선호하시나요? macOS 또는 Linux에 설치하세요</summary>

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

`orx up`은 `http://127.0.0.1:4791`에서 로컬 대시보드를 엽니다.

관리 대상 Mac에서는 `install.sh`로 설치한 CLI가 아직 서명되지 않아 기기 관리 정책에 의해 차단될 수 있습니다.
서명 및 공증된 데스크톱 앱을 사용하세요. 앱의
Settings → Updates → **Install the `orx` command**에서 터미널용 `orx`도 설치할 수 있습니다.

</details>

[openresearch.sh](https://openresearch.sh)에서 계정을 만들면 이메일로 소식을 받고
OpenResearch의 관리형 컴퓨팅을 사용할 수 있습니다.

## 작동 방식

**원하는 코딩 에이전트를 사용하세요.** OpenResearch는 다음 도구를 기본 지원합니다: <img src=".github/readme-assets/claude.svg" alt="" width="18" height="18" align="texttop" /> Claude Code,
<img src=".github/readme-assets/codex.svg" alt="" width="18" height="18" align="texttop" /> Codex,
<img src=".github/readme-assets/opencode.svg" alt="" width="18" height="18" align="texttop" /> OpenCode,
<img src=".github/readme-assets/cursor.svg" alt="" width="18" height="18" align="texttop" /> Cursor 및
<img src=".github/readme-assets/antigravity.svg" alt="" width="18" height="18" align="texttop" /> Google Antigravity.
OpenCode에서 다음 도구를 통해 [로컬 모델을 사용할 수도 있습니다](docs/local-models.md):
<img src=".github/readme-assets/lmstudio.svg" alt="" width="18" height="18" align="texttop" /> LM Studio,
<img src=".github/readme-assets/omlx.svg" alt="" width="18" height="18" align="texttop" /> oMLX,
<img src=".github/readme-assets/ollama.svg" alt="" width="18" height="18" align="texttop" /> Ollama 또는 사용자 지정 엔드포인트.

**최신 문헌을 근거로 연구 가설을 세우세요.** OpenResearch는 <img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv, <img src=".github/readme-assets/biorxiv.svg" alt="" width="18" height="18" align="absmiddle" /> bioRxiv, <img src=".github/readme-assets/pubmed.svg" alt="" width="18" height="18" align="absmiddle" /> PubMed와 연동되어 에이전트가 관련 논문을 찾고, 근거 있는 가설을 세우고, 실험을 기존 연구와 연결할 수 있습니다.

**컴퓨팅 환경을 선택하세요.** 로컬에서 실험을 실행하거나 SSH,
Slurm,
<img src=".github/readme-assets/kubernetes.svg" alt="" width="18" height="18" align="texttop" /> Kubernetes,
<img src=".github/readme-assets/modal.svg" alt="" width="18" height="18" align="texttop" /> Modal,
<img src=".github/readme-assets/thinking-machines.svg" alt="" width="18" height="18" align="texttop" /> Tinker,
<img src=".github/readme-assets/ray.svg" alt="" width="18" height="18" align="texttop" /> Ray,
<img src=".github/readme-assets/huggingface.svg" alt="" width="18" height="18" align="texttop" /> Hugging Face Jobs 및
OpenResearch의 관리형 컴퓨팅을 사용할 수 있습니다.

**에이전트가 모든 실험을 기억하게 하세요.** OpenResearch는 각 실험을 로컬 SQL 데이터베이스에 기록하고 로그, 코드,
산출물을 사용자 컴퓨터에 보관합니다. 에이전트는 이전 결과에서 배우고
다음에 무엇을 시도할지 결정할 수 있습니다. 데스크톱 앱에서는 에이전트가 한 작업을
쉽게 확인하고 각 결과의 근거를 살펴볼 수 있습니다.

**연구를 직접 보관하고 소유하세요.** 모든 작업을 로컬에서 실행할 수 있습니다. 코드나 에이전트 실행 기록을 수집하지 않습니다.
프로젝트, 대화, 실험, 로그, 산출물은 사용자의 컴퓨터에 저장되며
사용자가 관리합니다.

## 사용 분석

공식 릴리스 빌드는 임의의 설치 ID에 연결된 개략적인 사용 이벤트를 전송하며, 이를 끌 수 있습니다.
코드, 프롬프트, 파일 내용이나 경로, 저장소 이름, 토큰, 이메일,
프로젝트 및 실험 식별자는 포함하지 않습니다.

데스크톱 앱의 Settings에서 또는 `orx telemetry off` 명령으로 사용 분석을 끌 수 있습니다.
