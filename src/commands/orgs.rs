//! Lists organizations available for OpenResearch compute.

use crate::client::list_orgs;
use crate::error::{require_credentials, Result};
use crate::output::print_table;

pub async fn run(args: crate::OrgsArgs) -> Result<()> {
    let creds = require_credentials().await;
    let orgs = list_orgs(&creds).await?.orgs;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&orgs)?);
        return Ok(());
    }

    if orgs.is_empty() {
        println!("No organizations found for this account.");
        return Ok(());
    }

    let rows = orgs
        .iter()
        .map(|org| vec![org.id.clone(), org.name.clone()])
        .collect::<Vec<_>>();
    print_table(&["ID", "NAME"], &rows);
    Ok(())
}
