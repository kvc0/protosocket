
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = protocolsocket_server::Server::new()?;
    let port_nine_thousand = server.register_service_listener("127.0.0.1:9000")?;

    std::thread::spawn(move || { server.serve().expect("server must serve") });

    tokio::spawn(port_nine_thousand).await.expect("service must serve");
    Ok(())
}
