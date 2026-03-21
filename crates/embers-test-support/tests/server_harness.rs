use embers_test_support::{TestConnection, TestServer};

#[tokio::test]
async fn harness_starts_server_and_pings_it() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect to server");

    let payload = connection.ping("harness").await.expect("ping server");
    assert_eq!(payload, "harness");

    server.shutdown().await.expect("shutdown server");
}
