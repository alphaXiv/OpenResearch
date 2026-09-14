use clap::Args;

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// Override the API base URL (or set OPENRESEARCH_API_URL).
    #[arg(long = "api-url")]
    pub api_url: Option<String>,
}
