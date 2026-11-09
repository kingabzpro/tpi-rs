//! Ratatui terminal dashboard.
//!
//! Jobs run on the Tokio runtime and stream progress/result events to the UI.
//! Esc requests cooperative cancellation; q quits. The dashboard is read-only
//! with respect to brokerage accounts, apart from allowlisted CSV/JSON exports.

use crate::backtest::engine::{self, PreparedData};
use crate::backtest::types::{BacktestConfig, BacktestResult, FeeModel};
use crate::backtest::{BenchmarkReport, run_benchmark};
use crate::cli::Selection;
use crate::config::Config;
use crate::data::universe;
use crate::data::{Account, MarketData, Order, Position};
use crate::error::{Error, Result};
use crate::indicators::Series;
use crate::model::{Action, Horizon, Mode, Rating, Signal, StrategyMode};
use crate::report;
use crate::scan::{self, AnalysisOptions, ScanReport};
use crate::strategy::{EvalConfig, evaluate_fast};
use chrono::{Duration, NaiveDate};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Dataset, Gauge, GraphType, Paragraph, Row, Table,
    TableState, Tabs, Wrap,
};
use ratatui::{DefaultTerminal, Frame};
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration as StdDuration, Instant};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

pub struct TuiRequest {
    pub cfg: Config,
    pub selection: Selection,
    pub market: MarketData,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub offline: bool,
    pub refresh: bool,
    pub run_validation: bool,
    pub export: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum JobEvent {
    Status(String),
    Progress {
        phase: String,
        done: usize,
        total: usize,
    },
    Scan(Box<ScanReport>),
    Backtest(Box<BacktestResult>),
    Benchmark(Box<BenchmarkReport>),
    Portfolio {
        account: Account,
        positions: Vec<Position>,
        orders: Vec<Order>,
    },
    Done,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Signals,
    Chart,
    Rating,
    Portfolio,
    Backtest,
    Horizons,
    Help,
}

impl Tab {
    fn index(self) -> usize {
        match self {
            Self::Signals => 0,
            Self::Chart => 1,
            Self::Rating => 2,
            Self::Portfolio => 3,
            Self::Backtest => 4,
            Self::Horizons => 5,
            Self::Help => 6,
        }
    }

    fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Chart,
            2 => Self::Rating,
            3 => Self::Portfolio,
            4 => Self::Backtest,
            5 => Self::Horizons,
            6 => Self::Help,
            _ => Self::Signals,
        }
    }
}

#[derive(Debug, Clone)]
struct HorizonRow {
    symbol: String,
    short: Action,
    medium: Action,
    long: Action,
    rating: Rating,
}

#[derive(Debug, Clone, Copy)]
enum SortKey {
    Action,
    Strength,
    Rating,
    Symbol,
    Rsi,
    Atr,
}

impl SortKey {
    fn next(self) -> Self {
        match self {
            Self::Action => Self::Strength,
            Self::Strength => Self::Rating,
            Self::Rating => Self::Symbol,
            Self::Symbol => Self::Rsi,
            Self::Rsi => Self::Atr,
            Self::Atr => Self::Action,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Strength => "strength",
            Self::Rating => "rating",
            Self::Symbol => "symbol",
            Self::Rsi => "RSI",
            Self::Atr => "ATR%",
        }
    }
}

struct App {
    cfg: Config,
    selection: Selection,
    start: NaiveDate,
    end: NaiveDate,
    offline: bool,
    refresh: bool,
    run_validation: bool,
    export: Option<PathBuf>,
    tx: UnboundedSender<JobEvent>,
    rx: UnboundedReceiver<JobEvent>,
    job: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    running: bool,
    status: String,
    progress: Option<(String, usize, usize)>,
    tick: u64,
    started: Instant,
    tab: Tab,
    table_state: TableState,
    sort: SortKey,
    ascending: bool,
    report: Option<ScanReport>,
    backtest: Option<BacktestResult>,
    benchmark: Option<BenchmarkReport>,
    portfolio: Option<(Account, Vec<Position>, Vec<Order>)>,
    series_cache: Option<(String, Series)>,
    horizon_rows: Vec<HorizonRow>,
    ought_quit: bool,
}

pub async fn run(request: TuiRequest) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut app = App {
        cfg: request.cfg,
        selection: request.selection,
        start: request.start,
        end: request.end,
        offline: request.offline,
        refresh: request.refresh,
        run_validation: request.run_validation,
        export: request.export,
        tx,
        rx,
        job: None,
        cancel: Arc::new(AtomicBool::new(false)),
        running: false,
        status: "starting".to_string(),
        progress: None,
        tick: 0,
        started: Instant::now(),
        tab: Tab::Signals,
        table_state: TableState::default(),
        sort: SortKey::Action,
        ascending: true,
        report: None,
        backtest: None,
        benchmark: None,
        portfolio: None,
        series_cache: None,
        horizon_rows: Vec::new(),
        ought_quit: false,
    };
    app.spawn_initial(request.market);

    let mut terminal = ratatui::try_init()
        .map_err(|e| Error::validation(format!("failed to initialize terminal: {e}")))?;
    let result = app.event_loop(&mut terminal);
    app.cancel.store(true, Ordering::Relaxed);
    if let Some(job) = app.job.take() {
        job.abort();
    }
    ratatui::restore();
    result
}

impl App {
    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.ought_quit {
            self.tick += 1;
            while let Ok(event) = self.rx.try_recv() {
                self.handle_event(event);
            }
            terminal
                .draw(|frame| self.draw(frame))
                .map_err(|e| Error::validation(format!("terminal draw failed: {e}")))?;
            if event::poll(StdDuration::from_millis(60))
                .map_err(|e| Error::validation(format!("terminal poll failed: {e}")))?
                && let Event::Key(key) = event::read()
                    .map_err(|e| Error::validation(format!("terminal read failed: {e}")))?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key);
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Status(s) => self.status = s,
            JobEvent::Progress { phase, done, total } => {
                self.status = format!("{phase} {done}/{total}");
                self.progress = Some((phase, done, total));
            }
            JobEvent::Scan(report) => {
                self.status = format!(
                    "scan complete: {} signals, {} errors in {} ms",
                    report.signals.len(),
                    report.errors.len(),
                    report.elapsed_ms
                );
                self.horizon_rows = compute_horizon_rows(&report, &self.cfg, &self.selection);
                self.report = Some(*report);
                self.table_state.select(Some(0));
                self.series_cache = None;
            }
            JobEvent::Backtest(result) => {
                self.status = result.summary();
                self.backtest = Some(*result);
                self.tab = Tab::Backtest;
            }
            JobEvent::Benchmark(report) => {
                self.backtest = report.headlines.first().cloned();
                self.status = format!("benchmark complete in {} ms", report.elapsed_ms);
                self.benchmark = Some(*report);
                self.tab = Tab::Backtest;
            }
            JobEvent::Portfolio {
                account,
                positions,
                orders,
            } => self.portfolio = Some((account, positions, orders)),
            JobEvent::Done => {
                self.running = false;
                self.progress = None;
            }
            JobEvent::Failed(error) => {
                self.running = false;
                self.progress = None;
                self.status = format!("ERROR: {error}");
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.ought_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ought_quit = true
            }
            KeyCode::Tab => {
                self.tab = Tab::from_index((self.tab.index() + 1) % 7);
            }
            KeyCode::BackTab => {
                self.tab = Tab::from_index((self.tab.index() + 6) % 7);
            }
            KeyCode::Char(c @ '1'..='7') => {
                self.tab = Tab::from_index(c.to_digit(10).unwrap_or(1) as usize - 1);
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('s') => self.sort = self.sort.next(),
            KeyCode::Char('S') => self.ascending = !self.ascending,
            KeyCode::Enter => self.tab = Tab::Chart,
            KeyCode::Esc => self.cancel_job(),
            KeyCode::Char('r') => self.rerun(),
            KeyCode::Char('b') => self.spawn_backtest(),
            KeyCode::Char('a') => self.spawn_scan(),
            KeyCode::Char('e') => self.export_current(),
            KeyCode::Char('?') => self.tab = Tab::Help,
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: i64) {
        let Some(report) = &self.report else {
            return;
        };
        if report.signals.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i64;
        let next = (current + delta).clamp(0, report.signals.len() as i64 - 1) as usize;
        self.table_state.select(Some(next));
    }

    fn cancel_job(&mut self) {
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            self.status = "cancellation requested; waiting for the current step".to_string();
        }
    }

    fn rerun(&mut self) {
        if self.running {
            self.status = "a job is already running".to_string();
            return;
        }
        match self.selection.mode {
            Mode::Backtest => self.spawn_backtest(),
            Mode::Benchmark => self.spawn_benchmark(),
            _ => self.spawn_scan(),
        }
    }

    fn reset_job(&mut self) {
        self.cancel = Arc::new(AtomicBool::new(false));
        self.running = true;
        self.progress = None;
        self.status = "starting job".to_string();
        self.started = Instant::now();
    }

    fn spawn_initial(&mut self, market: MarketData) {
        match self.selection.mode {
            Mode::Backtest => self.spawn_backtest_with(Some(market)),
            Mode::Benchmark => self.spawn_benchmark_with(Some(market)),
            _ => self.spawn_scan_with(Some(market)),
        }
    }

    fn spawn_scan(&mut self) {
        self.spawn_scan_with(None);
    }

    fn spawn_scan_with(&mut self, market: Option<MarketData>) {
        if self.running {
            self.status = "a job is already running".to_string();
            return;
        }
        self.reset_job();
        let tx = self.tx.clone();
        let cfg = self.cfg.clone();
        let selection = self.selection.clone();
        let start = self.start;
        let end = self.end;
        let offline = self.offline;
        let refresh = self.refresh;
        let cancel = self.cancel.clone();
        let job = tokio::spawn(async move {
            let result = async {
                let market = match market {
                    Some(m) => m,
                    None => MarketData::new(&cfg, offline, refresh)?,
                };
                let _ = tx.send(JobEvent::Status("resolving symbols".into()));
                let (symbols, warnings) = if selection.top.is_some() {
                    universe::resolve_symbols(
                        market.client(),
                        &[],
                        selection.top,
                        market.offline(),
                        &cfg.backtest.universe,
                    )
                    .await?
                } else {
                    universe::resolve_symbols(
                        market.client(),
                        &selection.tickers,
                        None,
                        market.offline(),
                        &cfg.backtest.universe,
                    )
                    .await?
                };
                for w in warnings {
                    let _ = tx.send(JobEvent::Status(w));
                }
                let as_of = scan::completed_session_date(&market).await;
                let end = as_of.unwrap_or(end);
                let analysis_start = (end - Duration::days(500)).max(start);
                let tx_progress = tx.clone();
                let on_phase = move |phase: &str, done: usize, total: usize| {
                    let _ = tx_progress.send(JobEvent::Progress {
                        phase: phase.to_string(),
                        done,
                        total,
                    });
                };
                let progress = scan::Progress {
                    on_phase: &on_phase,
                };
                let options = AnalysisOptions {
                    horizon: selection.horizon,
                    strategy: selection.strategy,
                    risk: selection.risk,
                    vol_target_pct: selection.vol_target_pct,
                    risk_pct_override: selection.risk_pct_override,
                    equity: selection.capital,
                    config: &cfg,
                    as_of,
                    stale_bar_days: cfg.backtest.stale_bar_days,
                    cancel: Some(cancel.clone()),
                };
                let report = scan::analyze_market(
                    &market,
                    &symbols,
                    analysis_start,
                    end,
                    &options,
                    Some(&progress),
                )
                .await?;
                let _ = tx.send(JobEvent::Scan(Box::new(report)));
                send_portfolio(&market, &tx).await;
                Ok::<(), Error>(())
            }
            .await;
            finish(result, &tx);
        });
        self.job = Some(job);
    }

    fn spawn_backtest(&mut self) {
        self.spawn_backtest_with(None);
    }

    fn spawn_backtest_with(&mut self, market: Option<MarketData>) {
        if self.running {
            self.status = "a job is already running".to_string();
            return;
        }
        self.reset_job();
        let tx = self.tx.clone();
        let cfg = self.cfg.clone();
        let selection = self.selection.clone();
        let start = self.start;
        let end = self.end;
        let offline = self.offline;
        let refresh = self.refresh;
        let cancel = self.cancel.clone();
        let job = tokio::spawn(async move {
            let result = async {
                let market = match market {
                    Some(m) => m,
                    None => MarketData::new(&cfg, offline, refresh)?,
                };
                let _ = tx.send(JobEvent::Status("loading market data".into()));
                let (data, load) = load_prepared(&cfg, &market, &selection, start, end).await?;
                let _ = tx.send(JobEvent::Status(format!(
                    "loaded {} symbols, {} sessions{}",
                    data.universe.len(),
                    data.trading_days.len(),
                    if load.warnings.is_empty() {
                        String::new()
                    } else {
                        format!(" ({} data notes)", load.warnings.len())
                    }
                )));
                let mut base = backtest_config(&cfg, &selection, start, end);
                if offline {
                    base.label = format!("[SYNTHETIC DEMO] {}", base.label);
                }
                let params = cfg.horizon_params(selection.horizon);
                let tx_progress = tx.clone();
                let cancel_progress = cancel.clone();
                let progress = move |done: usize, total: usize| -> bool {
                    let _ = tx_progress.send(JobEvent::Progress {
                        phase: "backtest".into(),
                        done,
                        total,
                    });
                    !cancel_progress.load(Ordering::Relaxed)
                };
                let result = engine::run_with_progress(&data, &base, params, Some(&progress))?;
                let _ = tx.send(JobEvent::Backtest(Box::new(result)));
                send_portfolio(&market, &tx).await;
                Ok::<(), Error>(())
            }
            .await;
            finish(result, &tx);
        });
        self.job = Some(job);
    }

    fn spawn_benchmark(&mut self) {
        self.spawn_benchmark_with(None);
    }

    fn spawn_benchmark_with(&mut self, market: Option<MarketData>) {
        if self.running {
            self.status = "a job is already running".to_string();
            return;
        }
        self.reset_job();
        let tx = self.tx.clone();
        let cfg = self.cfg.clone();
        let selection = self.selection.clone();
        let start = self.start;
        let end = self.end;
        let offline = self.offline;
        let refresh = self.refresh;
        let run_validation = self.run_validation;
        let job = tokio::spawn(async move {
            let result = async {
                let market = match market {
                    Some(m) => m,
                    None => MarketData::new(&cfg, offline, refresh)?,
                };
                let _ = tx.send(JobEvent::Progress {
                    phase: "loading benchmark data".into(),
                    done: 0,
                    total: 0,
                });
                let (data, load) = load_prepared(&cfg, &market, &selection, start, end).await?;
                let mut base = backtest_config(&cfg, &selection, start, end);
                if offline {
                    base.label = format!("[SYNTHETIC DEMO] {}", base.label);
                }
                let params = cfg.horizon_params(selection.horizon);
                let _ = tx.send(JobEvent::Progress {
                    phase: "headline + walk-forward + stability".into(),
                    done: 0,
                    total: 0,
                });
                let report = run_benchmark(
                    &data,
                    &base,
                    params,
                    &cfg.backtest.walk_forward,
                    load.provenance,
                    run_validation,
                )?;
                let _ = tx.send(JobEvent::Benchmark(Box::new(report)));
                send_portfolio(&market, &tx).await;
                Ok::<(), Error>(())
            }
            .await;
            finish(result, &tx);
        });
        self.job = Some(job);
    }

    fn export_current(&mut self) {
        let dir = self.export.clone().unwrap_or_else(|| self.cfg.report_dir());
        let outcome = if let Some(benchmark) = &self.benchmark {
            report::export_benchmark(&dir, benchmark).map(|p| p.len())
        } else if let Some(backtest) = &self.backtest {
            let prefix = format!("tui-{}", report::timestamp_prefix());
            report::export_backtest(&dir, &prefix, backtest).map(|p| p.len())
        } else if let Some(scan) = &self.report {
            report::export_scan(&dir, scan).map(|p| p.len())
        } else {
            self.status = "nothing to export yet".to_string();
            return;
        };
        match outcome {
            Ok(n) => self.status = format!("exported {n} files to {}", dir.display()),
            Err(e) => self.status = format!("export failed: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(2),
            ])
            .split(area);
        self.draw_header(frame, layout[0]);
        self.draw_tabs(frame, layout[1]);
        match self.tab {
            Tab::Signals => self.draw_signals(frame, layout[2]),
            Tab::Chart => self.draw_chart(frame, layout[2]),
            Tab::Rating => self.draw_rating(frame, layout[2]),
            Tab::Portfolio => self.draw_portfolio(frame, layout[2]),
            Tab::Backtest => self.draw_backtest(frame, layout[2]),
            Tab::Horizons => self.draw_horizons(frame, layout[2]),
            Tab::Help => self.draw_help(frame, layout[2]),
        }
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(" q ", key_style()),
            Span::raw("quit  "),
            Span::styled(" Tab/1-7 ", key_style()),
            Span::raw("tabs  "),
            Span::styled(" j/k ", key_style()),
            Span::raw("move  "),
            Span::styled(" s/S ", key_style()),
            Span::raw("sort/direction  "),
            Span::styled(" Enter ", key_style()),
            Span::raw("chart  "),
            Span::styled(" Esc ", key_style()),
            Span::raw("cancel  "),
            Span::styled(" r ", key_style()),
            Span::raw("rerun  "),
            Span::styled(" b ", key_style()),
            Span::raw("backtest  "),
            Span::styled(" e ", key_style()),
            Span::raw("export  "),
            Span::styled(" ? ", key_style()),
            Span::raw("help"),
        ]))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(footer, layout[3]);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let source = self
            .report
            .as_ref()
            .map(|r| r.provenance.source.label())
            .or_else(|| self.benchmark.as_ref().map(|b| b.provenance.source.label()))
            .or(if self.offline {
                Some("SYNTHETIC DEMO")
            } else if self.backtest.is_some() {
                Some("backtest")
            } else {
                None
            })
            .unwrap_or("loading");
        let show_offline_tag = self.offline && source != "SYNTHETIC DEMO";
        let title = Line::from(vec![
            Span::styled(
                " TPI-RS ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " {} | {} | {} | risk {} | ${:.0} ",
                    self.selection.mode.label(),
                    self.selection.horizon.short_label(),
                    self.selection.strategy.label(),
                    self.selection.risk.label(),
                    self.selection.capital
                ),
                Style::default().fg(Color::White),
            ),
            Span::styled(format!("| {source} "), Style::default().fg(Color::Gray)),
            Span::styled(
                if show_offline_tag {
                    "| SYNTHETIC DEMO "
                } else {
                    ""
                },
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        frame.render_widget(block, area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(Paragraph::new(title), rows[0]);
        if let Some((phase, done, total)) = &self.progress {
            let (ratio, label) = if *total > 0 {
                (
                    (*done as f64 / *total as f64).clamp(0.0, 1.0),
                    format!(
                        "{phase} {done}/{total} {:>5.1}%",
                        (*done as f64 / *total as f64) * 100.0
                    ),
                )
            } else {
                let ratio = (self.tick % 100) as f64 / 100.0;
                (
                    ratio,
                    format!("{phase} {:.1}s", self.started.elapsed().as_secs_f64()),
                )
            };
            let gauge = Gauge::default()
                .ratio(ratio)
                .label(label)
                .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Rgb(18, 26, 36)));
            frame.render_widget(gauge, rows[1]);
        } else {
            let status = Line::from(vec![
                Span::styled("status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    self.status.clone(),
                    if self.status.starts_with("ERROR") {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
            ]);
            frame.render_widget(Paragraph::new(status), rows[1]);
        }
    }

    fn draw_tabs(&self, frame: &mut Frame, area: Rect) {
        let tabs = Tabs::new(vec![
            "Signals",
            "Chart",
            "Rating",
            "Portfolio",
            "Backtest",
            "Horizons",
            "Help",
        ])
        .select(self.tab.index())
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(tabs, area);
    }

    fn sorted_indices(&self) -> Vec<usize> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let mut indices: Vec<usize> = (0..report.signals.len()).collect();
        indices.sort_by(|&a, &b| {
            let sa = &report.signals[a];
            let sb = &report.signals[b];
            let cmp = match self.sort {
                SortKey::Action => action_rank(sa.action).cmp(&action_rank(sb.action)),
                SortKey::Strength => sa
                    .strength
                    .partial_cmp(&sb.strength)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Rating => sa.rating.rank().cmp(&sb.rating.rank()),
                SortKey::Symbol => sa.symbol.cmp(&sb.symbol),
                SortKey::Rsi => sa
                    .indicators
                    .rsi14
                    .unwrap_or(0.0)
                    .partial_cmp(&sb.indicators.rsi14.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Atr => sa
                    .indicators
                    .atr_pct
                    .unwrap_or(0.0)
                    .partial_cmp(&sb.indicators.atr_pct.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
            };
            if self.ascending { cmp } else { cmp.reverse() }
        });
        indices
    }

    fn selected_signal(&self) -> Option<&Signal> {
        let report = self.report.as_ref()?;
        let row = self.table_state.selected().unwrap_or(0);
        let indices = self.sorted_indices();
        indices.get(row).map(|&i| &report.signals[i])
    }

    fn draw_signals(&mut self, frame: &mut Frame, area: Rect) {
        let Some(report) = &self.report else {
            frame.render_widget(
                Paragraph::new("Loading market data and evaluating symbols...")
                    .block(Block::default().borders(Borders::ALL).title(" Signals "))
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        };
        let wide = area.width >= 110;
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        let table_area = if wide { columns[0] } else { area };
        let indices = self.sorted_indices();
        let header = Row::new(vec![
            "SYMBOL", "STRAT", "ACTION", "STR", "RATING", "CLOSE", "RSI", "ATR%", "STOP",
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let rows: Vec<Row> = indices
            .iter()
            .map(|&i| {
                let s = &report.signals[i];
                Row::new(vec![
                    Cell::from(s.symbol.clone()),
                    Cell::from(s.strategy.short_label()),
                    Cell::from(s.action.label()).style(action_style(s.action)),
                    Cell::from(format!("{:.0}", s.strength)),
                    Cell::from(s.rating.label()).style(rating_style(s.rating)),
                    Cell::from(format!("{:.2}", s.price)),
                    Cell::from(
                        s.indicators
                            .rsi14
                            .map(|v| format!("{v:.0}"))
                            .unwrap_or_else(|| "-".into()),
                    ),
                    Cell::from(
                        s.indicators
                            .atr_pct
                            .map(|v| format!("{v:.1}"))
                            .unwrap_or_else(|| "-".into()),
                    ),
                    Cell::from(format!("{:.2}", s.stop)),
                ])
            })
            .collect();
        let title = format!(
            " Signals ({}) sort: {}{} ",
            report.signals.len(),
            self.sort.label(),
            if self.ascending { " asc" } else { " desc" }
        );
        let row_count = rows.len();
        let table = Table::new(
            rows,
            [
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(4),
                Constraint::Length(11),
                Constraint::Length(8),
                Constraint::Length(4),
                Constraint::Length(5),
                Constraint::Length(8),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(24, 32, 44))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
        if row_count == 0 {
            self.table_state.select(None);
        } else {
            let selected = self.table_state.selected().unwrap_or(0).min(row_count - 1);
            self.table_state.select(Some(selected));
        }
        frame.render_stateful_widget(table, table_area, &mut self.table_state);

        if wide {
            let detail = self
                .selected_signal()
                .map(signal_detail_text)
                .unwrap_or_else(|| vec![Line::from("No signal selected")]);
            frame.render_widget(
                Paragraph::new(detail)
                    .block(Block::default().borders(Borders::ALL).title(" Detail "))
                    .wrap(Wrap { trim: true }),
                columns[1],
            );
        }
    }

    fn draw_chart(&mut self, frame: &mut Frame, area: Rect) {
        let Some(signal) = self.selected_signal().cloned() else {
            frame.render_widget(Paragraph::new("No signals loaded"), area);
            return;
        };
        let symbol = signal.symbol.clone();
        let horizon = self.selection.horizon;
        if self
            .series_cache
            .as_ref()
            .map(|(s, _)| s != &symbol)
            .unwrap_or(true)
            && let Some(report) = &self.report
            && let Some(bars) = report.bars.get(&symbol)
        {
            self.series_cache = Some((symbol.clone(), Series::compute(bars)));
        }
        let Some((_, series)) = &self.series_cache else {
            frame.render_widget(Paragraph::new(format!("No cached bars for {symbol}")), area);
            return;
        };
        if series.len() < 2 {
            frame.render_widget(Paragraph::new("Not enough bars for a chart"), area);
            return;
        }
        let take = series.len().min(250);
        let offset = series.len() - take;
        let close: Vec<(f64, f64)> = (offset..series.len())
            .map(|i| ((i - offset) as f64, series.close[i]))
            .collect();
        let ema50: Vec<(f64, f64)> = (offset..series.len())
            .filter_map(|i| series.ema50[i].map(|v| ((i - offset) as f64, v)))
            .collect();
        let ema200: Vec<(f64, f64)> = (offset..series.len())
            .filter_map(|i| series.ema200[i].map(|v| ((i - offset) as f64, v)))
            .collect();
        let don_high: Vec<(f64, f64)> = (offset..series.len())
            .filter_map(|i| series.don20_high[i].map(|v| ((i - offset) as f64, v)))
            .collect();
        let don_low: Vec<(f64, f64)> = (offset..series.len())
            .filter_map(|i| series.don20_low[i].map(|v| ((i - offset) as f64, v)))
            .collect();
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for dataset in [&close, &ema50, &ema200, &don_high, &don_low] {
            for (_, v) in dataset.iter() {
                min = min.min(*v);
                max = max.max(*v);
            }
        }
        if !min.is_finite() || !max.is_finite() || max <= min {
            frame.render_widget(Paragraph::new("Not enough chart data"), area);
            return;
        }
        let pad = (max - min) * 0.05;
        let lower = min - pad;
        let upper = max + pad;
        let step = (take / 6).max(1);
        let labels: Vec<String> = (0..take)
            .step_by(step)
            .map(|i| series.dates[offset + i].format("%m-%d").to_string())
            .collect();
        let datasets = vec![
            Dataset::default()
                .name("Close")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::Cyan))
                .data(&close),
            Dataset::default()
                .name("EMA50")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::Yellow))
                .data(&ema50),
            Dataset::default()
                .name("EMA200")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::Magenta))
                .data(&ema200),
            Dataset::default()
                .name("Don20 high")
                .marker(symbols::Marker::Dot)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::Green))
                .data(&don_high),
            Dataset::default()
                .name("Don20 low")
                .marker(symbols::Marker::Dot)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::Red))
                .data(&don_low),
        ];
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Length(8)])
            .split(area);
        let chart = Chart::new(datasets)
            .block(Block::default().borders(Borders::ALL).title(format!(
                " {symbol} | {} | close + EMA50/200 + Donchian20 ",
                horizon.short_label()
            )))
            .x_axis(
                Axis::default()
                    .bounds([0.0, (take.saturating_sub(1)) as f64])
                    .labels(labels),
            )
            .y_axis(Axis::default().bounds([lower, upper]).labels(vec![
                Line::from(format!("{lower:.1}")),
                Line::from(format!("{upper:.1}")),
            ]));
        frame.render_widget(chart, split[0]);
        let info = vec![
            Line::from(vec![
                Span::styled("Signal: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} {}", signal.action, signal.rating),
                    action_style(signal.action),
                ),
                Span::raw(format!(
                    "  strength {:.0}  close {:.2}  stop {:.2}  RSI {:.1}  ATR {:.2}%",
                    signal.strength,
                    signal.price,
                    signal.stop,
                    signal.indicators.rsi14.unwrap_or(0.0),
                    signal.indicators.atr_pct.unwrap_or(0.0)
                )),
            ]),
            Line::from(format!(
                "Bars: {}  |  Exit: {}",
                series.len(),
                crate::strategy::exit_policy(self.cfg.horizon_params(horizon))
            )),
            Line::from(
                signal
                    .reasons
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("  |  "),
            ),
        ];
        frame.render_widget(
            Paragraph::new(info)
                .block(Block::default().borders(Borders::ALL).title(" Context "))
                .wrap(Wrap { trim: true }),
            split[1],
        );
    }

    fn draw_rating(&mut self, frame: &mut Frame, area: Rect) {
        let Some(signal) = self.selected_signal() else {
            frame.render_widget(Paragraph::new("No signals loaded"), area);
            return;
        };
        let detail = &signal.rating_detail;
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        let scale = vec![
            Line::from("Five-level technical rating"),
            Line::from(""),
            Line::from(vec![
                Span::raw("Strong Sell  "),
                Span::styled(
                    if signal.rating == Rating::StrongSell {
                        "##########"
                    } else {
                        ".........."
                    },
                    Style::default().fg(Color::Red),
                ),
            ]),
            Line::from(vec![
                Span::raw("Sell         "),
                Span::styled(
                    if signal.rating == Rating::Sell {
                        "##########"
                    } else {
                        ".........."
                    },
                    Style::default().fg(Color::LightRed),
                ),
            ]),
            Line::from(vec![
                Span::raw("Neutral      "),
                Span::styled(
                    if signal.rating == Rating::Neutral {
                        "##########"
                    } else {
                        ".........."
                    },
                    Style::default().fg(Color::Yellow),
                ),
            ]),
            Line::from(vec![
                Span::raw("Buy          "),
                Span::styled(
                    if signal.rating == Rating::Buy {
                        "##########"
                    } else {
                        ".........."
                    },
                    Style::default().fg(Color::LightGreen),
                ),
            ]),
            Line::from(vec![
                Span::raw("Strong Buy   "),
                Span::styled(
                    if signal.rating == Rating::StrongBuy {
                        "##########"
                    } else {
                        ".........."
                    },
                    Style::default().fg(Color::Green),
                ),
            ]),
            Line::from(""),
            Line::from(format!(
                "{} | score {:+.2} | {} buy / {} neutral / {} sell",
                signal.rating,
                detail.score,
                detail.buy_votes,
                detail.neutral_votes,
                detail.sell_votes
            )),
            Line::from(""),
            Line::from(format!(
                "Moving averages: {} buy / {} neutral / {} sell",
                detail.ma_buy, detail.ma_neutral, detail.ma_sell
            )),
            Line::from(format!(
                "Oscillators:     {} buy / {} neutral / {} sell",
                detail.osc_buy, detail.osc_neutral, detail.osc_sell
            )),
            Line::from(""),
            Line::from("The rating is a consensus gauge, not a forecast."),
        ];
        frame.render_widget(
            Paragraph::new(scale)
                .block(Block::default().borders(Borders::ALL).title(" Rating "))
                .wrap(Wrap { trim: true }),
            columns[0],
        );
        let header = Row::new(vec!["COMPONENT", "VALUE", "VOTE"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let rows: Vec<Row> = detail
            .votes
            .iter()
            .map(|v| {
                Row::new(vec![
                    Cell::from(v.name.clone()),
                    Cell::from(
                        v.value
                            .map(|x| format!("{x:.2}"))
                            .unwrap_or_else(|| "-".into()),
                    ),
                    Cell::from(format!("{:?}", v.vote)).style(match v.vote {
                        crate::model::Vote::Buy => Style::default().fg(Color::Green),
                        crate::model::Vote::Sell => Style::default().fg(Color::Red),
                        crate::model::Vote::Neutral => Style::default().fg(Color::Yellow),
                    }),
                ])
            })
            .collect();
        let table = Table::new(
            rows,
            [
                Constraint::Length(20),
                Constraint::Length(12),
                Constraint::Length(8),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" 26 votes "));
        frame.render_widget(table, columns[1]);
    }

    fn draw_portfolio(&mut self, frame: &mut Frame, area: Rect) {
        if let Some((account, positions, orders)) = &self.portfolio {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("Equity ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("${:.2}  ", account.equity)),
                    Span::styled("Cash ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("${:.2}  ", account.cash)),
                    Span::styled("Buying power ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("${:.2}  ", account.available_cash())),
                    Span::styled("Day P&L ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:+.2}%", account.daily_pnl_pct()),
                        Style::default().fg(if account.daily_pnl_pct() >= 0.0 {
                            Color::Green
                        } else {
                            Color::Red
                        }),
                    ),
                ]),
                Line::from(format!(
                    "Account {} | status {} | PDT {} | day trades {}",
                    account.account_number,
                    account.status,
                    account.pattern_day_trader,
                    account.daytrade_count
                )),
                Line::from(""),
                Line::from(format!("Positions ({}):", positions.len())),
            ];
            for p in positions {
                lines.push(Line::from(format!(
                    "  {} qty {:.4} avg {:.2} current {:.2} unrealized {:+.2}",
                    p.symbol,
                    p.qty,
                    p.avg_entry_price,
                    p.current_price.unwrap_or(0.0),
                    p.unrealized_pl.unwrap_or(0.0)
                )));
            }
            if positions.is_empty() {
                lines.push(Line::from("  none"));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(format!("Open orders ({}):", orders.len())));
            for o in orders {
                lines.push(Line::from(format!(
                    "  {} {} {} qty {:?} filled {:.4} status {}",
                    o.id, o.side, o.symbol, o.qty, o.filled_qty, o.status
                )));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Broker portfolio (read-only) "),
                    )
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let mut lines = vec![
            Line::from(
                "No broker snapshot available (offline, no credentials, or request failed).",
            ),
            Line::from(""),
        ];
        if let Some(report) = &self.report {
            lines.push(Line::from(format!(
                "Analysis equity: ${:.2} | {} symbols | horizon {} | risk {}",
                report.equity,
                report.signals.len(),
                report.horizon.short_label(),
                report.risk.label()
            )));
            if let Some(best) = report.strongest() {
                lines.push(Line::from(format!(
                    "Strongest BUY: {} strength {:.0} | plan {} shares (${:.2}) | risk ${:.2}",
                    best.symbol,
                    best.strength,
                    best.suggested_shares,
                    best.suggested_notional,
                    best.risk_amount
                )));
                lines.push(Line::from(format!(
                    "Entry ref {:.2} | stop {:.2} ({:.2}%) | target {:.2}",
                    best.entry, best.stop, best.stop_distance_pct, best.target
                )));
            } else {
                lines.push(Line::from("No BUY candidate in the current scan."));
            }
        }
        if let Some(backtest) = &self.backtest {
            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "Backtest portfolio: ${:.2} -> ${:.2} | {} trades | max DD {:.2}%",
                backtest.initial_capital,
                backtest.metrics.final_equity,
                backtest.trades.len(),
                backtest.metrics.max_drawdown_pct
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" Portfolio "))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn draw_backtest(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(benchmark) = &self.benchmark {
            let mut lines = Vec::new();
            for headline in &benchmark.headlines {
                lines.push(Line::from(vec![
                    Span::styled(
                        headline.label.clone(),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        "  ${:.2} -> ${:.2}  return {:+.2}%  CAGR {:+.2}%  Sharpe {:.2}  maxDD {:.2}%  trades {}",
                        headline.initial_capital,
                        headline.metrics.final_equity,
                        headline.metrics.total_return_pct,
                        headline.metrics.cagr_pct,
                        headline.metrics.sharpe,
                        headline.metrics.max_drawdown_pct,
                        headline.metrics.trades
                    )),
                ]));
                for w in &headline.windows {
                    lines.push(Line::from(format!(
                        "   {:<25} {:>8.2}%  DD {:>6.2}%  trades {:>3}",
                        w.label, w.return_pct, w.max_drawdown_pct, w.trades
                    )));
                }
                lines.push(Line::from(""));
            }
            if let Some(wf) = &benchmark.walk_forward {
                lines.push(Line::from(vec![Span::styled(
                    "Walk-forward",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]));
                for (i, fold) in wf.folds.iter().enumerate() {
                    lines.push(Line::from(format!(
                        "  F{} {}..{} | {} | IS Sharpe {:.2} | OOS {:+.2}% Sharpe {:.2} DD {:.2}%",
                        i + 1,
                        fold.train_start,
                        fold.train_end,
                        fold.selected,
                        fold.in_sample.sharpe,
                        fold.out_of_sample.total_return_pct,
                        fold.out_of_sample.sharpe,
                        fold.out_of_sample.max_drawdown_pct
                    )));
                }
                if let Some(oos) = &wf.oos {
                    lines.push(Line::from(format!(
                        "  Untouched OOS {}..{} | selected {} | return {:+.2}% | Sharpe {:.2} | DD {:.2}%",
                        oos.test_start,
                        oos.test_end,
                        oos.selected,
                        oos.out_of_sample.total_return_pct,
                        oos.out_of_sample.sharpe,
                        oos.out_of_sample.max_drawdown_pct
                    )));
                }
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" 3-Year Benchmark "),
                    )
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let Some(result) = &self.backtest else {
            let message = if self.running {
                "Backtest running... (Esc cancels)"
            } else {
                "No backtest loaded. Press b to run one."
            };
            frame.render_widget(
                Paragraph::new(message)
                    .block(Block::default().borders(Borders::ALL).title(" Backtest ")),
                area,
            );
            return;
        };
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(7), Constraint::Min(6)])
            .split(area);
        let lower = if split[1].width >= 100 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
                .split(split[1])
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(split[1])
        };
        let summary = vec![
            Line::from(vec![Span::styled(
                result.summary(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(format!(
                "Vol {:.2}% | exposure {:.0}% | win rate {:.1}% | PF {:.2} | fees ${:.2} | slippage/spread included",
                result.metrics.annual_volatility_pct,
                result.metrics.exposure_pct,
                result.metrics.win_rate_pct,
                result.metrics.profit_factor,
                result.metrics.total_fees
            )),
            Line::from(format!(
                "Period {}..{} | {} sessions | universe {} | {}",
                result.start,
                result.end,
                result.trading_days,
                result.universe_size,
                result.data_note
            )),
        ];
        frame.render_widget(
            Paragraph::new(summary)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Backtest summary "),
                )
                .wrap(Wrap { trim: true }),
            split[0],
        );
        let header = Row::new(vec![
            "WINDOW", "RETURN", "MAX DD", "TRADES", "WIN%", "FINAL",
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let rows: Vec<Row> = result
            .windows
            .iter()
            .map(|w| {
                Row::new(vec![
                    Cell::from(w.label.clone()),
                    Cell::from(format!("{:+.2}%", w.return_pct)).style(return_style(w.return_pct)),
                    Cell::from(format!("{:.2}%", w.max_drawdown_pct)),
                    Cell::from(w.trades.to_string()),
                    Cell::from(format!("{:.1}%", w.win_rate_pct)),
                    Cell::from(format!("${:.2}", w.final_equity)),
                ])
            })
            .collect();
        let table = Table::new(
            rows,
            [
                Constraint::Length(25),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(11),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Six-month windows "),
        );
        frame.render_widget(table, lower[0]);

        let trade_header = Row::new(vec!["SYMBOL", "ENTRY", "EXIT", "NET%", "REASON"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let trade_rows: Vec<Row> = result
            .trades
            .iter()
            .rev()
            .take(14)
            .map(|t| {
                Row::new(vec![
                    Cell::from(t.symbol.clone()),
                    Cell::from(t.entry_date.to_string()),
                    Cell::from(t.exit_date.to_string()),
                    Cell::from(format!("{:+.2}", t.net_return_pct))
                        .style(return_style(t.net_return_pct)),
                    Cell::from(t.exit_reason.chars().take(16).collect::<String>()),
                ])
            })
            .collect();
        let trade_table = Table::new(
            trade_rows,
            [
                Constraint::Length(7),
                Constraint::Length(11),
                Constraint::Length(11),
                Constraint::Length(7),
                Constraint::Length(18),
            ],
        )
        .header(trade_header)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " Recent trades (last 14 of {}) ",
            result.trades.len()
        )));
        frame.render_widget(trade_table, lower[1]);
    }

    fn draw_horizons(&mut self, frame: &mut Frame, area: Rect) {
        if self.horizon_rows.is_empty() {
            frame.render_widget(
                Paragraph::new("Run a scan to compute short, medium and long recommendations.")
                    .block(Block::default().borders(Borders::ALL).title(" Horizons "))
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let header = Row::new(vec![
            "SYMBOL",
            "SHORT (1-5d)",
            "MEDIUM (1-4w)",
            "LONG (1-6m)",
            "RATING",
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let rows: Vec<Row> = self
            .horizon_rows
            .iter()
            .map(|r| {
                Row::new(vec![
                    Cell::from(r.symbol.clone()),
                    Cell::from(r.short.label()).style(action_style(r.short)),
                    Cell::from(r.medium.label()).style(action_style(r.medium)),
                    Cell::from(r.long.label()).style(action_style(r.long)),
                    Cell::from(r.rating.label()).style(rating_style(r.rating)),
                ])
            })
            .collect();
        let table = Table::new(
            rows,
            [
                Constraint::Length(10),
                Constraint::Length(14),
                Constraint::Length(15),
                Constraint::Length(15),
                Constraint::Length(14),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Horizon-specific recommendations (independent parameter sets) "),
        );
        frame.render_widget(table, area);
    }

    fn draw_help(&mut self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(vec![Span::styled(
                "TPI-RS dashboard",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from("1-7 or Tab      switch tabs"),
            Line::from("j/k, Up/Down    move signal selection"),
            Line::from("s / S           cycle sort column / reverse order"),
            Line::from("Enter           open the selected symbol chart"),
            Line::from("r               rerun the current job"),
            Line::from("b / a           run a backtest / scan"),
            Line::from("Esc             cancel scan/backtest; benchmark finishes its phase"),
            Line::from("e               export CSV/JSON to the reports directory"),
            Line::from("q               quit"),
            Line::from(""),
            Line::from("Tabs: Signals, Chart, Rating, Portfolio, Backtest, Horizons, Help."),
            Line::from("Horizons shows short/medium/long actions with separate parameter sets."),
            Line::from(""),
            Line::from(
                "Rules: threatening breakout with EMA50/200 trend, RSI/ATR filters, ATR trailing stop.",
            ),
            Line::from("Signals are rule-based conditions, not price forecasts. No AI/ML."),
            Line::from("Live trading is intentionally unavailable in the dashboard."),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" Help "))
                .wrap(Wrap { trim: true }),
            area,
        );
    }
}

fn finish(result: Result<()>, tx: &UnboundedSender<JobEvent>) {
    match result {
        Ok(()) => {
            let _ = tx.send(JobEvent::Done);
        }
        Err(Error::Cancelled) => {
            let _ = tx.send(JobEvent::Failed("cancelled".to_string()));
        }
        Err(e) => {
            let _ = tx.send(JobEvent::Failed(e.to_string()));
        }
    }
}

async fn send_portfolio(market: &MarketData, tx: &UnboundedSender<JobEvent>) {
    let Some(client) = market.client() else {
        return;
    };
    if let (Ok(account), Ok(positions), Ok(orders)) = (
        client.account().await,
        client.positions().await,
        client.orders("open").await,
    ) {
        let _ = tx.send(JobEvent::Portfolio {
            account,
            positions,
            orders,
        });
    }
}

async fn load_prepared(
    cfg: &Config,
    market: &MarketData,
    selection: &Selection,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<(PreparedData, crate::data::LoadResult)> {
    let list = if !selection.tickers.is_empty() {
        universe::parse_tickers(&selection.tickers)
    } else {
        selection.universe.clone()
    };
    if list.is_empty() {
        return Err(Error::validation("backtest universe is empty"));
    }
    let benchmarks = cfg.backtest.benchmark_symbols.clone();
    let mut symbols = list.clone();
    for b in &benchmarks {
        if !symbols.contains(b) {
            symbols.push(b.clone());
        }
    }
    let warm_days = ((cfg.backtest.warmup_trading_days.max(220) as i64) * 2).max(450);
    let warm_start = start - Duration::days(warm_days);
    let load = market.load(&symbols, warm_start, end).await?;
    let data = PreparedData::new(&load.bars, &list, &benchmarks)?;
    Ok((data, load))
}

fn backtest_config(
    cfg: &Config,
    selection: &Selection,
    start: NaiveDate,
    end: NaiveDate,
) -> BacktestConfig {
    let mut config = BacktestConfig {
        start,
        end,
        initial_capital: selection.capital,
        horizon: selection.horizon,
        strategy: if selection.strategy == StrategyMode::Compare {
            StrategyMode::Tpi
        } else {
            selection.strategy
        },
        risk: selection.risk,
        vol_target_pct: selection.vol_target_pct,
        risk_pct_override: selection.risk_pct_override,
        fees: FeeModel {
            commission_bps: cfg.backtest.commission_bps,
            spread_bps: cfg.backtest.spread_bps,
            slippage_bps: cfg.backtest.slippage_bps,
            sec_fee_bps: cfg.backtest.sec_fee_bps,
            finra_taf_per_share: cfg.backtest.finra_taf_per_share,
        },
        allow_fractional: cfg.backtest.allow_fractional,
        risk_free_rate_pct: cfg.strategy.risk_free_rate_pct,
        stale_bar_days: cfg.backtest.stale_bar_days,
        min_history_bars: cfg.strategy.min_history_bars,
        max_hold_days_override: cfg.backtest.max_hold_days_override,
        label: format!("TUI {} {}..{}", selection.horizon.short_label(), start, end),
    };
    if selection.strategy == StrategyMode::Compare {
        config.label.push_str(" [TPI leg]");
    }
    config
}

fn compute_horizon_rows(
    report: &ScanReport,
    cfg: &Config,
    selection: &Selection,
) -> Vec<HorizonRow> {
    let min_bars = cfg.strategy.min_history_bars;
    let strategy = if selection.strategy == StrategyMode::Compare {
        StrategyMode::Tpi
    } else {
        selection.strategy
    };
    let mut rows: Vec<HorizonRow> = report
        .bars
        .par_iter()
        .filter_map(|(symbol, bars)| {
            if bars.len() < min_bars {
                return None;
            }
            let series = Series::compute(bars);
            let i = series.len() - 1;
            let mut actions = [Action::Hold; 3];
            let mut rating = Rating::Neutral;
            for (idx, horizon) in [Horizon::Short, Horizon::Medium, Horizon::Long]
                .into_iter()
                .enumerate()
            {
                let eval = EvalConfig {
                    horizon,
                    strategy,
                    risk: selection.risk,
                    equity: selection.capital,
                    cash: selection.capital,
                    allow_fractional: cfg.backtest.allow_fractional,
                    require_rating_buy: cfg.strategy.require_rating_buy,
                    volume_ratio_min: cfg.strategy.volume_ratio_min,
                    min_history_bars: cfg.strategy.min_history_bars,
                    long_only: cfg.strategy.long_only,
                    vol_target_pct: 0.0,
                    risk_pct_override: None,
                };
                if let Ok(signal) =
                    evaluate_fast(symbol, &series, i, cfg.horizon_params(horizon), eval)
                {
                    actions[idx] = signal.action;
                    if horizon == Horizon::Medium {
                        rating = signal.rating;
                    }
                }
            }
            Some(HorizonRow {
                symbol: symbol.clone(),
                short: actions[0],
                medium: actions[1],
                long: actions[2],
                rating,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    rows
}

fn signal_detail_text(s: &Signal) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", s.symbol),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(s.action.label().to_string(), action_style(s.action)),
            Span::raw(format!("  strength {:.0}/100", s.strength)),
        ]),
        Line::from(format!(
            "{} | {} | as of {}",
            s.rating,
            s.strategy.short_label(),
            s.as_of
        )),
        Line::from(format!(
            "Close {:.2} | EMA50 {:.2} | EMA200 {:.2}",
            s.price,
            s.indicators.ema50.unwrap_or(0.0),
            s.indicators.ema200.unwrap_or(0.0)
        )),
        Line::from(format!(
            "RSI {:.1} | ATR {:.2}% | ADX {:.1} | vol {:.2}x",
            s.indicators.rsi14.unwrap_or(0.0),
            s.indicators.atr_pct.unwrap_or(0.0),
            s.indicators.adx14.unwrap_or(0.0),
            s.indicators.volume_ratio.unwrap_or(0.0)
        )),
        Line::from(format!(
            "Entry {:.2} | Stop {:.2} ({:.2}%) | Target {:.2}",
            s.entry, s.stop, s.stop_distance_pct, s.target
        )),
        Line::from(format!(
            "Score {:.0} = {:.0} trend + {:.0} breakout + {:.0} momentum + {:.0} confirm + {:.0} vol",
            s.components.total(),
            s.components.trend,
            s.components.breakout,
            s.components.momentum,
            s.components.confirmation,
            s.components.volatility
        )),
        Line::from(format!(
            "Panel: {} buy / {} neutral / {} sell",
            s.rating_detail.buy_votes, s.rating_detail.neutral_votes, s.rating_detail.sell_votes
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Why:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    for reason in s.reasons.iter().take(4) {
        lines.push(Line::from(format!("- {reason}")));
    }
    if !s.warnings.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Warnings:",
            Style::default().fg(Color::Yellow),
        )));
        for warning in s.warnings.iter().take(4) {
            lines.push(Line::from(format!("- {warning}")));
        }
    }
    lines
}

fn action_rank(action: Action) -> u8 {
    match action {
        Action::Buy => 0,
        Action::Hold => 1,
        Action::Sell => 2,
    }
}

fn action_style(action: Action) -> Style {
    match action {
        Action::Buy => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        Action::Hold => Style::default().fg(Color::Yellow),
        Action::Sell => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn rating_style(rating: Rating) -> Style {
    match rating {
        Rating::StrongBuy => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        Rating::Buy => Style::default().fg(Color::LightGreen),
        Rating::Neutral => Style::default().fg(Color::Yellow),
        Rating::Sell => Style::default().fg(Color::LightRed),
        Rating::StrongSell => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn return_style(value: f64) -> Style {
    if value >= 0.0 {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    }
}

fn key_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::DarkGray)
}
