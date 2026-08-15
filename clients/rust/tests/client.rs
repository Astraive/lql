use lql_client::{Client, ConnectionConfig, ErrorCategory, QueryValue};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

#[test]
fn query_uses_scoped_route_and_bearer_precedence() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 8192];
        let size = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..size]);
        let request = request.to_ascii_lowercase();
        assert!(request.contains("authorization: bearer api-key"));
        assert!(request.contains("x-loza-env: prod"));
        assert!(request.contains("x-loza-service: cli"));
        let response = concat!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
            "{\"columns\":[{\"name\":\"event_id\",\"type\":\"string\"}],\"rows\":[{\"event_id\":\"evt-1\"}],\"duration_ms\":2,\"row_count\":1}"
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    let client = Client::new(ConnectionConfig {
        endpoint: Some(format!("http://{address}")),
        collector: Some("demo".into()),
        api_key: Some("api-key".into()),
        username: Some("user".into()),
        password: Some("pass".into()),
        env: Some("prod".into()),
        service: Some("cli".into()),
        ..Default::default()
    })
    .unwrap();
    let result = client
        .query(
            "from events | where event_id = $id",
            [("id".into(), QueryValue::new("string", "evt-1"))]
                .into_iter()
                .collect(),
            10,
        )
        .unwrap();
    assert_eq!(result.row_count, 1);
    server.join().unwrap();
}

#[test]
fn invalid_configuration_has_stable_category() {
    let error = match Client::new(ConnectionConfig {
        endpoint: Some("http://remote.example".into()),
        ..Default::default()
    }) {
        Ok(_) => panic!("expected invalid configuration"),
        Err(error) => error,
    };
    assert_eq!(error.category, ErrorCategory::InvalidConfiguration);
}
