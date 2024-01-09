const SOURCE: &str =
    "https://raw.githubusercontent.com/pradt2/always-online-stun/master/valid_hosts.txt";

#[tokio::main]
async fn main() {
    let response = reqwest::get(SOURCE).await.unwrap().text().await.unwrap();

    println!("/// List of public STUN servers");
    println!("pub const STUN_SERVERS: &[&str] = &[");

    for line in response.lines() {
        println!("    {line:?},");
    }

    println!("];");
    println!();
}
