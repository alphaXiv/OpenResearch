//! Independent literature retrieval primitives for the main agent.

use crate::client::{
    discover_openalex, discover_papers_by_embedding, discover_papers_by_keyword, LitHit,
    OpenAlexDiscoveryOptions, PaperDiscoveryOptions, BIORXIV_SOURCE_ID,
};
use crate::error::{anyhow, Result};
use crate::LitSource;

pub async fn run(args: crate::DiscoverArgs) -> Result<()> {
    let disabled = crate::config::disabled_lit_sources();
    let results: Vec<LitHit> = match args.command {
        crate::DiscoverCommand::Keyword(args) => {
            ensure_source_enabled(LitSource::Alphaxiv, &disabled)?;
            discover_papers_by_keyword(&args.query, alphaxiv_options(&args))
                .await?
                .into_iter()
                .take(args.limit as usize)
                .map(LitHit::from)
                .collect()
        }
        crate::DiscoverCommand::Embedding(args) => {
            ensure_source_enabled(LitSource::Alphaxiv, &disabled)?;
            discover_papers_by_embedding(&args.query, alphaxiv_options(&args))
                .await?
                .into_iter()
                .take(args.limit as usize)
                .map(LitHit::from)
                .collect()
        }
        crate::DiscoverCommand::Openalex(args) => {
            ensure_source_enabled(LitSource::Openalex, &disabled)?;
            discover_openalex(&args.query, openalex_options(&args, None)).await?
        }
        crate::DiscoverCommand::Biorxiv(args) => {
            ensure_source_enabled(LitSource::Biorxiv, &disabled)?;
            discover_openalex(
                &args.query,
                openalex_options(&args, Some(BIORXIV_SOURCE_ID)),
            )
            .await?
        }
    };

    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

fn alphaxiv_options(args: &crate::DiscoverySearchArgs) -> PaperDiscoveryOptions<'_> {
    PaperDiscoveryOptions {
        published_after: args.published_after.as_deref(),
        published_before: args.published_before.as_deref(),
        prioritize: args.prioritize.as_str(),
    }
}

fn openalex_options<'a>(
    args: &'a crate::DiscoverySearchArgs,
    source_filter: Option<&'a str>,
) -> OpenAlexDiscoveryOptions<'a> {
    OpenAlexDiscoveryOptions {
        limit: args.limit,
        published_after: args.published_after.as_deref(),
        published_before: args.published_before.as_deref(),
        prioritize: args.prioritize.as_str(),
        source_filter,
    }
}

fn ensure_source_enabled(source: LitSource, disabled: &[String]) -> Result<()> {
    if disabled.iter().any(|disabled| disabled == source.as_str()) {
        return Err(anyhow!(
            "{} is disabled by your OpenResearch literature-source configuration. Re-enable it to discover {} papers.",
            source.display_name(),
            source.display_name(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_source_enabled;
    use crate::LitSource;

    #[test]
    fn respects_disabled_discovery_source() {
        let error = ensure_source_enabled(LitSource::Alphaxiv, &["alphaxiv".to_string()])
            .expect_err("disabled alphaXiv should reject retrieval");
        assert!(error.to_string().contains("alphaXiv is disabled"));
        let error = ensure_source_enabled(LitSource::Biorxiv, &["biorxiv".to_string()])
            .expect_err("disabled bioRxiv should reject retrieval");
        assert!(error.to_string().contains("bioRxiv is disabled"));
        ensure_source_enabled(LitSource::Openalex, &[])
            .expect("enabled OpenAlex should permit retrieval");
    }
}
