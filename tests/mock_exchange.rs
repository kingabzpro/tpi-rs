//! Integration tests against a local HTTP mock of the Alpaca REST API.
//! The mock simulates pagination, auth headers, retries, account/positions,
//! order submission, lookup by client id, cancellation and flattening.

use chrono::NaiveDate;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tpi_rs::config::AlpacaConfig;
use tpi_rs::data::alpaca::{AlpacaClient, OrderRequest};

#[derive(Debug, Clone)]
struct RequestRecord {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

struct MockState {
    requests: Mutex<Vec<RequestRecord>>,
    clock_failures_left: AtomicUsize,
}

struct MockServer {
    base: String,
    state: Arc<MockState>,
    stop: Arc<AtomicBool>,
}

impl MockServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(MockState {
            requests: Mutex::new(Vec::new()),
            clock_failures_left: AtomicUsize::new(1),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let state2 = state.clone();
        let stop2 = stop.clone();
        tokio::spawn(async move {
            while !stop2.load(Ordering::Relaxed) {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let state = state2.clone();
                tokio::spawn(async move {
                    let Some(req) = read_request(&mut socket).await else {
                        return;
                    };
                    let record = req.clone();
                    state.requests.lock().await.push(record);
                    let (status, body) = route(&req, &state).await;
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\nretry-after: 0\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            base: format!("http://{addr}"),
            state,
            stop,
        }
    }

    fn client(&self) -> AlpacaClient {
        let cfg = AlpacaConfig {
            feed: "iex".into(),
            adjustment: "all".into(),
            data_url: self.base.clone(),
            max_retries: 2,
            timeout_secs: 5,
            ..AlpacaConfig::default()
        };
        AlpacaClient::with_urls(
            &cfg,
            self.base.clone(),
            "test-key".into(),
            "test-secret".into(),
        )
        .unwrap()
    }

    async fn requests(&self) -> Vec<RequestRecord> {
        self.state.requests.lock().await.clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<RequestRecord> {
    let mut buffer = Vec::new();
    let header_end;
    loop {
        let mut chunk = [0u8; 1024];
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buffer) {
            header_end = pos;
            break;
        }
    }
    let header_text = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let mut chunk = [0u8; 1024];
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(
        &buffer[body_start..(body_start + content_length).min(buffer.len())],
    )
    .to_string();
    Some(RequestRecord {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|w| w == b"\r\n\r\n")
}

fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

async fn route(req: &RequestRecord, state: &MockState) -> (&'static str, String) {
    let path = path_only(&req.path);
    match (req.method.as_str(), path) {
        ("GET", "/v2/clock") => {
            if state.clock_failures_left.swap(0, Ordering::SeqCst) > 0 {
                (
                    "500 Internal Server Error",
                    json!({"message":"transient mock failure"}).to_string(),
                )
            } else {
                (
                    "200 OK",
                    json!({
                        "is_open": false,
                        "next_open": "2026-11-09T14:30:00Z",
                        "next_close": "2026-11-09T21:00:00Z",
                        "timestamp": "2026-11-08T15:00:00Z"
                    })
                    .to_string(),
                )
            }
        }
        ("GET", "/v2/calendar") => (
            "200 OK",
            json!([{
                "date": "2026-11-09",
                "open": "09:30",
                "close": "16:00",
                "settlement_date": "2026-11-10"
            }])
            .to_string(),
        ),
        ("GET", "/v2/account") => (
            "200 OK",
            json!({
                "id": "acct-1",
                "account_number": "12345",
                "status": "ACTIVE",
                "currency": "USD",
                "cash": "10000.00",
                "buying_power": "20000.00",
                "non_marginable_buying_power": "9500.00",
                "equity": "10100.00",
                "last_equity": "10000.00",
                "long_market_value": "100.00",
                "portfolio_value": "10100.00",
                "pattern_day_trader": false,
                "trading_blocked": false,
                "account_blocked": false,
                "daytrade_count": 0
            })
            .to_string(),
        ),
        ("GET", "/v2/positions") => (
            "200 OK",
            json!([{
                "symbol": "AAPL",
                "qty": "1.5",
                "qty_available": "1.5",
                "avg_entry_price": "100.00",
                "side": "long",
                "market_value": "150.00",
                "cost_basis": "150.00",
                "unrealized_pl": "0.00",
                "unrealized_plpc": "0.0",
                "current_price": "100.00"
            }])
            .to_string(),
        ),
        ("GET", "/v2/orders") => ("200 OK", "[]".to_string()),
        ("DELETE", "/v2/orders") => ("207 Multi-Status", "{}".to_string()),
        ("POST", "/v2/orders") => {
            let body: serde_json::Value = serde_json::from_str(&req.body).unwrap_or(json!({}));
            let cid = body
                .get("client_order_id")
                .and_then(|v| v.as_str())
                .unwrap_or("missing");
            let qty = body.get("qty").and_then(|v| v.as_str()).unwrap_or("1");
            order_json(cid, qty, "filled")
        }
        ("GET", "/v2/orders:by_client_order_id") => {
            let cid = req
                .path
                .split("client_order_id=")
                .nth(1)
                .unwrap_or("cid")
                .split('&')
                .next()
                .unwrap_or("cid");
            order_json(cid, "2", "filled")
        }
        ("DELETE", "/v2/positions/AAPL") => order_json("close-1", "1.5", "filled"),
        ("GET", "/v2/stocks/bars") => {
            if req.path.contains("page_token=page2") {
                (
                    "200 OK",
                    json!({
                        "bars": {
                            "AAPL": [{"t":"2026-10-05T04:00:00Z","o":101.0,"h":103.0,"l":100.0,"c":102.0,"v":1100.0}]
                        },
                        "next_page_token": null
                    })
                    .to_string(),
                )
            } else {
                (
                    "200 OK",
                    json!({
                        "bars": {
                            "AAPL": [
                                {"t":"2026-10-02T04:00:00Z","o":100.0,"h":102.0,"l":99.0,"c":101.0,"v":1000.0},
                                {"t":"2026-10-01T04:00:00Z","o":99.0,"h":101.0,"l":98.0,"c":100.0,"v":900.0}
                            ],
                            "MSFT": [
                                {"t":"2026-10-01T04:00:00Z","o":200.0,"h":202.0,"l":199.0,"c":201.0,"v":500.0}
                            ]
                        },
                        "next_page_token": "page2"
                    })
                    .to_string(),
                )
            }
        }
        ("DELETE", p) if p.starts_with("/v2/orders/") => ("204 No Content", String::new()),
        _ => (
            "404 Not Found",
            json!({"message": format!("unmocked {} {}", req.method, req.path)}).to_string(),
        ),
    }
}

fn order_json(cid: &str, qty: &str, status: &str) -> (&'static str, String) {
    (
        "200 OK",
        json!({
            "id": format!("order-{cid}"),
            "client_order_id": cid,
            "symbol": "AAPL",
            "side": "buy",
            "qty": qty,
            "filled_qty": qty,
            "filled_avg_price": "101.25",
            "status": status,
            "type": "market",
            "submitted_at": "2026-11-08T15:00:00Z",
            "filled_at": "2026-11-08T15:00:01Z"
        })
        .to_string(),
    )
}

#[tokio::test]
async fn bars_paginate_sort_and_send_auth() {
    let server = MockServer::start().await;
    let client = server.client();
    let bars = client
        .daily_bars(
            &["AAPL".to_string(), "MSFT".to_string()],
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bars.get("AAPL").unwrap().len(), 3);
    assert_eq!(
        bars.get("AAPL")
            .unwrap()
            .iter()
            .map(|b| b.date)
            .collect::<Vec<_>>(),
        vec![
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 5).unwrap()
        ]
    );
    assert_eq!(bars.get("MSFT").unwrap().len(), 1);
    let requests = server.requests().await;
    assert!(requests.iter().any(|r| r.path.contains("page_token=page2")));
    let bars_req = requests
        .iter()
        .find(|r| r.path.starts_with("/v2/stocks/bars"))
        .unwrap();
    assert_eq!(bars_req.headers.get("apca-api-key-id").unwrap(), "test-key");
    assert_eq!(
        bars_req.headers.get("apca-api-secret-key").unwrap(),
        "test-secret"
    );
}

#[tokio::test]
async fn account_positions_submit_and_lookup() {
    let server = MockServer::start().await;
    let client = server.client();
    let account = client.account().await.unwrap();
    assert!((account.equity - 10_100.0).abs() < 1e-9);
    assert!((account.available_cash() - 9_500.0).abs() < 1e-9);
    assert!((account.daily_pnl_pct() - 1.0).abs() < 1e-9);
    let positions = client.positions().await.unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].symbol, "AAPL");

    let req = OrderRequest::market("AAPL", 2.0, "buy");
    let cid = req.client_order_id.clone();
    let order = client.submit_order(&req).await.unwrap();
    assert_eq!(order.status, "filled");
    assert_eq!(order.filled_qty, 2.0);
    let found = client.order_by_client_id(&cid).await.unwrap().unwrap();
    assert_eq!(found.client_order_id, cid);
    let requests = server.requests().await;
    let post = requests
        .iter()
        .find(|r| r.method == "POST" && path_only(&r.path) == "/v2/orders")
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&post.body).unwrap();
    assert_eq!(body["side"], "buy");
    assert_eq!(body["qty"], "2");
    assert_eq!(body["client_order_id"], cid);
}

#[tokio::test]
async fn transient_500_is_retried_for_get() {
    let server = MockServer::start().await;
    let client = server.client();
    let clock = client.clock().await.unwrap();
    assert!(!clock.is_open);
    let calendar = client
        .calendar(
            NaiveDate::from_ymd_opt(2026, 11, 9).unwrap(),
            NaiveDate::from_ymd_opt(2026, 11, 15).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(calendar.len(), 1);
    assert_eq!(
        calendar[0].date,
        NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
    );
    let requests = server.requests().await;
    let clocks = requests
        .iter()
        .filter(|r| path_only(&r.path) == "/v2/clock")
        .count();
    assert!(clocks >= 2, "expected a retry, saw {clocks} clock requests");
}

#[tokio::test]
async fn emergency_flatten_cancels_and_closes() {
    let server = MockServer::start().await;
    let client = server.client();
    let report = client.emergency_flatten().await.unwrap();
    assert!(report.orders_cancelled);
    assert_eq!(report.closed.len(), 1);
    assert!(report.errors.is_empty());
    let requests = server.requests().await;
    assert!(
        requests
            .iter()
            .any(|r| r.method == "DELETE" && path_only(&r.path) == "/v2/orders")
    );
    assert!(
        requests
            .iter()
            .any(|r| r.method == "DELETE" && path_only(&r.path) == "/v2/positions/AAPL")
    );
}
