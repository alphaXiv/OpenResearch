use std::io::BufRead;
fn main() {
    let exe=std::env::current_exe().unwrap();
    let agent=exe.file_stem().unwrap().to_str().unwrap();
    let arg=std::env::args().nth(1).unwrap_or_default();
    match (agent,arg.as_str()) {
        ("claude","--version")=>println!("2.1.212 (Claude Code)"),
        ("claude","auth")=>{println!(r#"{{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}}"#);std::process::exit(1)},
        ("codex","--version")=>println!("codex-cli 0.144.0"),
        ("codex","app-server")=>{let mut line=String::new();std::io::stdin().lock().read_line(&mut line).unwrap(); if line.contains("initialize") { println!(r#"{{"id":1,"error":{{"code":-32001,"message":"private marker used only by the synthetic agent"}}}}"#); } },
        _=>std::process::exit(1)
    }
}
