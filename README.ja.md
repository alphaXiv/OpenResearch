<div align="center">

<h1><img src=".github/readme-assets/openresearch.svg" alt="" width="56" align="absmiddle" /> OpenResearch</h1>

研究エージェントのためのローカル中心のハーネスとワークスペース。

<p><em>an <a href="https://alphaxiv.org"><img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv</a> project</em></p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest"><img src="https://img.shields.io/github/v/release/alphaXiv/OpenResearch?style=flat-square" alt="Latest release" /></a>
<a href="https://github.com/alphaXiv/OpenResearch/blob/main/LICENSE"><img src="https://img.shields.io/github/license/alphaXiv/OpenResearch?style=flat-square" alt="License" /></a>
</p>

<p><a href="README.md">English</a> · <a href="README.es.md">Español</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">简体中文</a> · <a href="README.ja.md">日本語</a></p>

<p><img src=".github/readme-assets/openresearch-screenshot.png" alt="OpenResearch desktop app showing a research agent conversation and experiment logs" width="680" /></p>

<hr />

<p>コパイロットを探している方にも、自律研究ツールを探している方にも、OpenResearch は作業を加速します。 文献のレビュー、仮説の構築、実験の実行、研究成果の作成を行う研究エージェントを起動しましょう。</p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-macos-dark.svg"><img src=".github/readme-assets/download-macos.svg" alt="Download OpenResearch for macOS" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-windows-dark.svg"><img src=".github/readme-assets/download-windows.svg" alt="Download OpenResearch for Windows（ベータ版）" width="220" height="44" /></picture></a>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-linux-dark.svg"><img src=".github/readme-assets/download-linux.svg" alt="Download OpenResearch for Linux" width="220" height="44" /></picture></a>
</p>

<p>
<a href="https://openresearch.sh/docs"><img src=".github/readme-assets/action-documentation.svg" alt="Documentation" width="132" height="24" /></a><img src=".github/readme-assets/action-separator.svg" alt=" · " width="12" height="24" />
<a href="https://github.com/alphaXiv/OpenResearch/releases"><img src=".github/readme-assets/action-releases.svg" alt="Releases" width="78" height="24" /></a>
</p>

<p><sub>macOS 11+ · Windows ベータ版には次が必要です： <a href="docs/windows.md">Git for Windows</a> · <a href="docs/linux.md">Linux</a> アプリには glibc 2.35 以上が必要です</sub></p>

<p><a href="https://trendshift.io/repositories/89363"><img src="https://trendshift.io/api/badge/repositories/89363" alt="GitHub Trending: #1 Repository of the Day" width="250" height="55" /></a>
<a href="https://trendshift.io/repositories/89363?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-89363" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/89363/daily?language=Rust" alt="alphaXiv/OpenResearch | Trendshift" width="250" height="55" /></a></p>

</div>

## はじめに

**OpenResearch デスクトップアプリの利用をおすすめします。** お使いのプラットフォーム向けにダウンロードしてください：

- [macOS](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch.dmg)
- [Windows（ベータ版）](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-Setup.exe)
- Linux: [x86_64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-x86_64.AppImage) · [ARM64](https://github.com/alphaXiv/OpenResearch/releases/latest/download/OpenResearch-aarch64.AppImage)

<details>
<summary>スタンドアロンの CLI を使いたい場合：macOS または Linux にインストール</summary>

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

`orx up` は `http://127.0.0.1:4791` のローカルダッシュボードを開きます。

管理対象の Mac では、`install.sh` でインストールした CLI が未署名のため、デバイス管理ポリシーによってブロックされる場合があります。
署名・公証済みのデスクトップアプリを使用してください。アプリの
Settings → Updates → **Install the `orx` command** から、ターミナル用の `orx` もインストールできます。

</details>

[openresearch.sh](https://openresearch.sh) でアカウントを作成すると、メールで最新情報を受け取り、
OpenResearch のマネージドコンピュートを利用できます。

## 仕組み

**お気に入りのコーディングエージェントを使う。** OpenResearch は次のツールにネイティブ対応しています： <img src=".github/readme-assets/claude.svg" alt="" width="18" height="18" align="texttop" /> Claude Code,
<img src=".github/readme-assets/codex.svg" alt="" width="18" height="18" align="texttop" /> Codex,
<img src=".github/readme-assets/opencode.svg" alt="" width="18" height="18" align="texttop" /> OpenCode,
<img src=".github/readme-assets/cursor.svg" alt="" width="18" height="18" align="texttop" /> Cursor、
<img src=".github/readme-assets/antigravity.svg" alt="" width="18" height="18" align="texttop" /> Google Antigravity.
OpenCode では、次のツールを通じて[ローカルモデルを使う](docs/local-models.md)こともできます：
<img src=".github/readme-assets/lmstudio.svg" alt="" width="18" height="18" align="texttop" /> LM Studio,
<img src=".github/readme-assets/omlx.svg" alt="" width="18" height="18" align="texttop" /> oMLX,
<img src=".github/readme-assets/ollama.svg" alt="" width="18" height="18" align="texttop" /> Ollama、またはカスタムエンドポイント。

**最新の文献に基づいて研究仮説を立てる。** OpenResearch は <img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv、<img src=".github/readme-assets/biorxiv.svg" alt="" width="18" height="18" align="absmiddle" /> bioRxiv、<img src=".github/readme-assets/pubmed.svg" alt="" width="18" height="18" align="absmiddle" /> PubMed と連携し、エージェントが関連論文を見つけ、根拠のある仮説を立て、実験を既存の研究と結び付けられるようにします。

**実行環境を選ぶ。** ローカルで実験を実行するほか、SSH、
Slurm,
<img src=".github/readme-assets/kubernetes.svg" alt="" width="18" height="18" align="texttop" /> Kubernetes,
<img src=".github/readme-assets/modal.svg" alt="" width="18" height="18" align="texttop" /> Modal,
<img src=".github/readme-assets/thinking-machines.svg" alt="" width="18" height="18" align="texttop" /> Tinker,
<img src=".github/readme-assets/ray.svg" alt="" width="18" height="18" align="texttop" /> Ray,
<img src=".github/readme-assets/huggingface.svg" alt="" width="18" height="18" align="texttop" /> Hugging Face Jobs、
OpenResearch のマネージドコンピュートを利用できます。

**すべての実験をエージェントの記憶にする。** OpenResearch は各実験をローカルの SQL データベースに記録し、ログ、コード、
成果物をあなたのマシンに保存します。エージェントは過去の結果から学び、
次に何を試すかを判断できます。デスクトップアプリでは、エージェントが行った作業と
各結果の根拠を簡単に確認できます。

**研究を手元に保管し、自分で所有する。** すべてローカルで実行できます。コードやエージェントの実行履歴は収集しません。
プロジェクト、会話、実験、ログ、成果物はあなたのマシンに保存され、
あなたが管理します。

## 利用状況の分析

公式リリースビルドは、ランダムなインストール ID に紐づく大まかな利用イベントを送信します。送信は無効にできます。
コード、プロンプト、ファイルの内容やパス、リポジトリ名、トークン、メールアドレス、
プロジェクトや実験の識別子は含まれません。

デスクトップアプリの Settings、または `orx telemetry off` で利用状況の分析を無効にできます。
