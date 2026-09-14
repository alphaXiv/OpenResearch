use clap::{Args, Subcommand, ValueEnum};

/// Which corpus a literature command searches or reads from.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum LitSource {
    /// alphaXiv (arXiv corpus: CS, math, physics, stats — the default).
    Alphaxiv,
    /// OpenAlex (general scholarly graph across all disciplines).
    Openalex,
    /// bioRxiv biology preprints (searched via OpenAlex, fetched via bioRxiv).
    Biorxiv,
}

impl LitSource {
    /// Lowercase wire name used to enforce against the Settings disable-set.
    /// Matches the `--source` flag values (clap `rename_all = "lower"`) and the
    /// `LitHit.source` JSON labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            LitSource::Alphaxiv => "alphaxiv",
            LitSource::Openalex => "openalex",
            LitSource::Biorxiv => "biorxiv",
        }
    }

    /// Human-facing name for error/UI text.
    pub fn display_name(&self) -> &'static str {
        match self {
            LitSource::Alphaxiv => "alphaXiv",
            LitSource::Openalex => "OpenAlex",
            LitSource::Biorxiv => "bioRxiv",
        }
    }
}

#[derive(Args, Debug)]
pub struct DiscoverArgs {
    #[command(subcommand)]
    pub command: DiscoverCommand,
}

#[derive(Subcommand, Debug)]
pub enum DiscoverCommand {
    /// alphaXiv full-text BM25 retrieval with match snippets.
    Keyword(DiscoverySearchArgs),
    /// alphaXiv semantic title/abstract retrieval with similarity/popularity reranking.
    Embedding(DiscoverySearchArgs),
    /// OpenAlex scholarly-graph search across disciplines.
    Openalex(DiscoverySearchArgs),
    /// bioRxiv preprint search through OpenAlex's bioRxiv source index.
    Biorxiv(DiscoverySearchArgs),
}

#[derive(Args, Debug)]
pub struct DiscoverySearchArgs {
    /// Exact keyword query or semantic description, depending on the strategy.
    pub query: String,
    /// Include papers first published on or after this date (YYYY-MM-DD).
    #[arg(long = "published-after")]
    pub published_after: Option<String>,
    /// Include papers first published on or before this date (YYYY-MM-DD). Older
    /// or narrow embedding windows can return a thin candidate set.
    #[arg(long = "published-before")]
    pub published_before: Option<String>,
    /// Ranking policy after topical relevance is accounted for.
    #[arg(long, value_enum, default_value = "default")]
    pub prioritize: DiscoveryPriority,
    /// Maximum results to emit (default 15). alphaXiv uses its fixed server-side candidate pool.
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u32).range(1..=200))]
    pub limit: u32,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum DiscoveryPriority {
    Historical,
    Default,
    Recency,
    Popular,
}

impl DiscoveryPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Historical => "historical",
            Self::Default => "default",
            Self::Recency => "recency",
            Self::Popular => "popular",
        }
    }
}

#[derive(Args, Debug)]
pub struct PaperArgs {
    /// Paper id: an arXiv id / URL (alphaXiv), a DOI (bioRxiv `10.1101/…` or any
    /// other), or an OpenAlex `W…` id. The source is auto-detected.
    pub id: String,
    /// Force the source instead of auto-detecting it from the id.
    #[arg(long, value_enum)]
    pub source: Option<LitSource>,
    /// Fetch the full extracted paper text instead of the report (alphaXiv only;
    /// OpenAlex/bioRxiv have no extracted full text and point you at the PDF).
    #[arg(long)]
    pub full: bool,
}
