use lql_client::{Client, ConnectionConfig, ErrorCategory, QueryValue};
use std::collections::HashMap;
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
        assert!(request.contains("\"connection\":\"analytics\""));
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
        service: Some("cli".into()),
        env: Some("prod".into()),
        database_connection: Some("analytics".into()),
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
fn oversized_response_is_rejected_without_buffering_the_whole_body() {
    const LIMIT: usize = 64;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 8192];
        let _ = stream.read(&mut buffer).unwrap();
        // Announce a body much larger than the configured limit but send only
        // LIMIT + 1 bytes before closing. A client that buffers the whole body
        // sees a truncated body (a transport error); one that bounds its read
        // stops at LIMIT + 1 bytes and rejects on the size limit instead.
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            LIMIT * 8
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&vec![b'x'; LIMIT + 1]).unwrap();
    });
    let client = Client::new(ConnectionConfig {
        endpoint: Some(format!("http://{address}")),
        collector: Some("demo".into()),
        max_response_bytes: Some(LIMIT),
        ..Default::default()
    })
    .unwrap();
    let error = match client.query("from events", HashMap::new(), 10) {
        Ok(_) => panic!("oversized response must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.category, ErrorCategory::MalformedResponse);
    assert!(error.message.contains("exceeds the configured size limit"));
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

#[test]
fn rejects_client_database_dsns() {
    for dsn in ["postgres://user:pass@db/loza", "duckdb:///tmp/loza.db"] {
        let error = match Client::new(ConnectionConfig {
            dsn: Some(dsn.into()),
            ..Default::default()
        }) {
            Ok(_) => panic!("database DSN must not be accepted"),
            Err(error) => error,
        };
        assert_eq!(error.category, ErrorCategory::InvalidConfiguration);
    }
}

#[test]
fn rejects_api_key_over_plaintext_http_to_remote_host() {
    let error = match Client::new(ConnectionConfig {
        endpoint: Some("http://remote.example".into()),
        collector: Some("demo".into()),
        api_key: Some("secret-api-key".into()),
        ..Default::default()
    }) {
        Ok(_) => panic!("API key over plaintext HTTP must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.category, ErrorCategory::InvalidConfiguration);
    assert!(error.message.contains("TLS"), "{}", error.message);
}

#[test]
fn rejects_api_key_from_plaintext_dsn_to_remote_host() {
    let error = match Client::new(ConnectionConfig {
        dsn: Some("loza://remote.example/demo?tls=false".into()),
        api_key: Some("secret-api-key".into()),
        ..Default::default()
    }) {
        Ok(_) => panic!("API key over plaintext DSN must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.category, ErrorCategory::InvalidConfiguration);
    assert!(error.message.contains("TLS"), "{}", error.message);
}

#[test]
fn rejects_basic_auth_over_plaintext_http_to_remote_host() {
    let error = match Client::new(ConnectionConfig {
        endpoint: Some("http://remote.example".into()),
        collector: Some("demo".into()),
        username: Some("user".into()),
        password: Some("pass".into()),
        ..Default::default()
    }) {
        Ok(_) => panic!("basic auth over plaintext HTTP must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.category, ErrorCategory::InvalidConfiguration);
    assert!(
        error.message.contains("basic authentication requires TLS"),
        "{}",
        error.message
    );
}

#[test]
fn allows_api_key_over_tls_to_remote_host() {
    assert!(Client::new(ConnectionConfig {
        endpoint: Some("https://remote.example".into()),
        collector: Some("demo".into()),
        api_key: Some("secret-api-key".into()),
        ..Default::default()
    })
    .is_ok());
}

#[test]
fn allows_unauthenticated_plaintext_http_to_remote_host() {
    assert!(Client::new(ConnectionConfig {
        endpoint: Some("http://remote.example".into()),
        collector: Some("demo".into()),
        ..Default::default()
    })
    .is_ok());
}
