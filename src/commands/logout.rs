//! Takes no args. Clears local credentials, then forgets the alphaXiv tokens,
//! revoking them at alphaXiv, and prints a confirmation, or reports "Not logged
//! in." when there are neither.

use crate::config::{clear_credentials, load_credentials};
use crate::error::Result;

pub async fn run() -> Result<()> {
    if load_credentials().await?.is_none() && !crate::alphaxiv::signed_in() {
        println!("Not logged in.");
        return Ok(());
    }
    clear_credentials().await?;
    crate::alphaxiv::sign_out().await?;
    println!("\u{2713} Logged out. Local credentials removed.");
    Ok(())
}
