//! Takes no args. Forgets the alphaXiv tokens, revoking them at alphaXiv, then
//! clears local credentials and prints a confirmation, or reports "Not logged
//! in." when there are none.

use crate::config::{clear_credentials, load_credentials};
use crate::error::Result;

pub async fn run() -> Result<()> {
    let creds = load_credentials().await?;
    crate::alphaxiv::sign_out().await?;
    if creds.is_none() {
        println!("Not logged in.");
        return Ok(());
    }
    clear_credentials().await?;
    println!("\u{2713} Logged out. Local credentials removed.");
    Ok(())
}
