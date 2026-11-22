//! Live operator surface: a tiny std-only HTTP server inside the AI DLL that
//! serves a dashboard (JSON snapshot + map PNG) and periodically writes
//! `status.json` / `board.png` into the AI's writable data dir
//! (`<write-dir>/AI/Skirmish/ZKTDAI/<version>/`).
//!
//! Minimized port of zkgbai-rs's `operator` module (`server.rs` + `board.rs`):
//! no async runtime and no HTTP framework — one thread per connection — and a
//! hand-rolled software canvas encoded with the `png` crate, so the AI needs
//! no GUI stack at all.
//!
//! Enable with the `operator` AI option (`on`, a port, or `host:port`) or the
//! `ZKTDAI_OPERATOR` env var (tools/headless/run.ps1 `-Viewer` sets it);
//! default port 7460, the next 32 ports are tried while it is taken.
//!
//! Routes: `GET /` dashboard, `GET /snapshot` JSON, `GET /board.png` map PNG,
//! `GET /health` liveness. Positions are map elmos (x east, z south).

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use springai::{Callback, Map, Vec3};

/// First port tried when the option says just `on`.
pub const DEFAULT_PORT: u16 = 7460;
/// Ports tried after the requested one while it is taken.
const PORT_TRIES: u16 = 32;
const MAX_HEADER: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 16;
/// Read timeout on an idle connection.
const IDLE: Duration = Duration::from_secs(30);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Map the `operator` option / `ZKTDAI_OPERATOR` env var to a listen spec.
/// `None` = disabled.
fn parse_spec(raw: Option<String>) -> Option<String> {
    let v = raw?.trim().to_lowercase();
    match v.as_str() {
        "" | "off" | "none" | "false" | "0" => None,
        "on" | "true" | "yes" => Some(format!("127.0.0.1:{DEFAULT_PORT}")),
        s if s.chars().all(|c| c.is_ascii_digit()) => Some(format!("127.0.0.1:{s}")),
        s => Some(s.to_owned()),
    }
}

/// Option first, env var as fallback (headless runs set the env var).
fn resolve_spec(cb: Callback) -> Option<String> {
    let opt = cb.option("operator").filter(|v| !v.trim().is_empty());
    let env = std::env::var("ZKTDAI_OPERATOR").ok();
    parse_spec(opt.or(env))
}

// ── HTTP server (std only, one thread per connection) ─────────────────────

/// What the AI thread publishes for the server threads to serve.
pub struct Shared {
    pub shutdown: AtomicBool,
    pub frame: AtomicI32,
    pub snapshot: Mutex<Option<String>>,
    pub board: Mutex<Option<Vec<u8>>>,
    pub name: String,
    pub version: String,
}

pub struct Server {
    addr: String,
    shared: Arc<Shared>,
    threads: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
    acceptor: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Listen on `spec` (`host:port`), or on the next free port of the
    /// following few.
    pub fn start(spec: &str, shared: Arc<Shared>) -> io::Result<Server> {
        let (host, port) = match spec.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), p.parse::<u16>().map_err(|e| io::Error::other(e.to_string()))?),
            None => return Err(io::Error::other(format!("bad address '{spec}'"))),
        };
        let mut last = io::Error::other("no port tried");
        for p in port..port.saturating_add(PORT_TRIES) {
            let addrs: Vec<_> = (host.as_str(), p).to_socket_addrs()?.collect();
            match TcpListener::bind(&addrs[..]) {
                Ok(listener) => {
                    listener.set_nonblocking(true)?;
                    let addr = listener.local_addr()?.to_string();
                    let threads = Arc::new(Mutex::new(Vec::new()));
                    let (s, t) = (shared.clone(), threads.clone());
                    let acceptor = std::thread::Builder::new()
                        .name("zktdai-operator".into())
                        .spawn(move || accept_loop(listener, s, t))?;
                    return Ok(Server { addr, shared, threads, acceptor: Some(acceptor) });
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Stop accepting, end all connections and wait for their threads (the
    /// engine may unload our library right after release).
    pub fn stop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        if let Some(a) = self.acceptor.take() {
            let _ = a.join();
        }
        let threads = std::mem::take(&mut *lock(&self.threads));
        for t in threads {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, threads: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>) {
    while !shared.shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut list = lock(&threads);
                list.retain(|t| !t.is_finished());
                if list.len() >= MAX_CONNECTIONS {
                    let _ = respond(&stream, 503, "Too Many Requests", "text/plain", b"too many connections");
                    continue;
                }
                let s = shared.clone();
                if let Ok(t) =
                    std::thread::Builder::new().name("zktdai-operator-conn".into()).spawn(move || serve(stream, &s))
                {
                    list.push(t);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

struct Request {
    method: String,
    path: String,
}

/// Serve one request, then close (every response says `Connection: close`).
fn serve(stream: TcpStream, shared: &Arc<Shared>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IDLE));
    let _ = stream.set_nodelay(true);
    let mut buf = Vec::new();
    if let Ok(Some(req)) = read_request(&stream, &mut buf) {
        route(&stream, &req, shared);
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn read_request(mut stream: &TcpStream, buf: &mut Vec<u8>) -> io::Result<Option<Request>> {
    let start = Instant::now();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(i) = find(buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEADER || start.elapsed() > IDLE {
            return Ok(None);
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_owned();
    let target = first.next().unwrap_or("/").to_owned();
    // The dashboard cache-busts the board with a query (`/board.png?t=…`).
    let path = target.split_once('?').map(|(p, _)| p).unwrap_or(&target).to_owned();
    Ok(Some(Request { method, path }))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn route(stream: &TcpStream, req: &Request, shared: &Arc<Shared>) {
    let path = req.path.trim_end_matches('/');
    match (req.method.as_str(), path) {
        ("GET", "" | "/index.html" | "/dashboard") => {
            let _ = respond(stream, 200, "OK", "text/html; charset=utf-8", DASHBOARD.as_bytes());
        }
        ("GET", "/snapshot") => {
            let body = lock(&shared.snapshot).clone().unwrap_or_else(|| "null".into());
            let _ = respond(stream, 200, "OK", "application/json", body.as_bytes());
        }
        ("GET", "/board.png") => match lock(&shared.board).clone() {
            Some(png) => {
                let _ = respond(stream, 200, "OK", "image/png", &png);
            }
            None => {
                let _ = respond(stream, 404, "Not Found", "application/json", br#"{"error":"no board yet"}"#);
            }
        },
        ("GET", "/health") => {
            let body = format!(
                "{{\"ok\":true,\"frame\":{},\"name\":\"{}\",\"version\":\"{}\"}}",
                shared.frame.load(Ordering::Relaxed),
                shared.name,
                shared.version
            );
            let _ = respond(stream, 200, "OK", "application/json", body.as_bytes());
        }
        _ => {
            let body = format!("{{\"error\":\"no route {} {}\"}}", req.method, req.path);
            let _ = respond(stream, 404, "Not Found", "application/json", body.as_bytes());
        }
    }
}

fn respond(stream: &TcpStream, status: u16, text: &str, content_type: &str, body: &[u8]) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    // `&TcpStream` implements `Write`.
    let mut w = stream;
    w.write_all(&out)
}

const DASHBOARD: &str = r#"<!doctype html>
<html><head><meta charset=utf-8><title>ZKTDAI operator</title>
<style>
body{background:#111;color:#ccc;font:13px monospace;margin:8px}
h3{margin:4px 0}
img{image-rendering:pixelated;width:640px;border:1px solid #333;background:#000}
pre{background:#1a1a1a;padding:8px;max-height:42vh;overflow:auto;white-space:pre-wrap}
table{border-collapse:collapse}td,th{padding:1px 12px 1px 0;border-bottom:1px solid #222;text-align:left}
a{color:#8af}
</style></head><body>
<h3>ZKTDAI operator — <span id=h>…</span></h3>
<img id=b src="/board.png" alt="board">
<div>legend: mex ring <span style=color:#58c>blue</span>=ours <span style=color:#f46>red</span>=enemy
<span style=color:#999>grey</span>=blocked <span style=color:#eee>white</span>=free ·
units: <span style=color:#ee6>yellow</span>=commander <span style=color:#3e6>green</span>=worker
<span style=color:#48f>blue square</span>=building <span style=color:#0ff>cyan</span>=army ·
enemies <span style=color:#f28>red</span> · <span style=color:#f8f>magenta</span>=front</div>
<h3>Projects</h3>
<table id=proj><tr><th>project</th><th>units dedicated</th><th>state</th></tr></table>
<pre id=s>…</pre>
<a href="/snapshot">snapshot</a> · <a href="/board.png">board.png</a> · <a href="/health">health</a>
<script>
const b=document.getElementById('b'),s=document.getElementById('s'),h=document.getElementById('h'),p=document.getElementById('proj');
function esc(x){return String(x).replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));}
function renderProj(j){
  const rows=[['<th>project</th><th>units dedicated</th><th>state</th>']];
  if(j.commander){
    let t='commander: '+j.commander.opening;
    if(j.commander.mex_spot!=null)t+=' → mex spot #'+j.commander.mex_spot;
    rows.push([t,'1 (commander)','']);
  }
  (j.jobs||[]).forEach(g=>rows.push(['job: '+g.job,g.units.length+' con ['+g.units.join(',')+']','']));
  (j.projects||[]).forEach(x=>{
    let who=[];
    if(x.builder!=null)who.push('builder u='+x.builder);
    if(x.target!=null)who.push('building u='+x.target);
    rows.push(['project: '+x.name, who.join(', ')||'—', x.state+' · '+x.kind+' · reserved '+Math.round(x.reserved)+'m']);
  });
  p.innerHTML=rows.map(r=>'<tr>'+r.map(c=>'<td>'+esc(c)+'</td>').join('')+'</tr>').join('');
}
async function tick(){
  try{const r=await fetch('/health');const j=await r.json();h.textContent='frame '+j.frame+' · '+j.name+' '+j.version;}catch(e){h.textContent='(not up yet)';}
  try{const r=await fetch('/snapshot');const j=await r.json();s.textContent=JSON.stringify(j,null,1);renderProj(j);}catch(e){}
  b.src='/board.png?t='+Date.now();
}
tick();setInterval(tick,1000);
</script></body></html>
"#;

/// The operator: owns the server and the published state, writes the
/// status files. All methods are cheap and lock-free-ish; called from the
/// AI thread only.
pub struct Operator {
    server: Option<Server>,
    shared: Arc<Shared>,
    dir: Option<String>,
}

impl Operator {
    /// Start the operator if the `operator` option / `ZKTDAI_OPERATOR` env
    /// var asks for it.
    pub fn start(cb: Callback) -> Operator {
        let dir = cb.writeable_dir();
        let shared = Arc::new(Shared {
            shutdown: AtomicBool::new(false),
            frame: AtomicI32::new(0),
            snapshot: Mutex::new(None),
            board: Mutex::new(None),
            name: "ZKTDAI".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        });
        let server = match resolve_spec(cb) {
            Some(spec) => match Server::start(&spec, shared.clone()) {
                Ok(server) => {
                    let url = format!("http://{}/", server.addr());
                    cb.log(&format!(
                        "[zktdai] operator: listening on {url} (snapshot {url}snapshot, board {url}board.png)"
                    ));
                    if let Some(d) = &dir {
                        let port = server.addr().rsplit_once(':').map(|(_, p)| p).unwrap_or("");
                        let disc = format!(
                            "{{\"url\":\"{url}\",\"port\":\"{port}\",\"team\":{},\"ally_team\":{}}}\n",
                            cb.game().my_team(),
                            cb.game().my_ally_team()
                        );
                        let _ = std::fs::write(format!("{d}/operator.json"), disc);
                    }
                    Some(server)
                }
                Err(e) => {
                    cb.log(&format!("[zktdai] operator: cannot listen on {spec}: {e}"));
                    None
                }
            },
            None => None,
        };
        Operator { server, shared, dir }
    }

    pub fn enabled(&self) -> bool {
        self.server.is_some()
    }

    /// Called every frame.
    pub fn tick(&self, frame: i32) {
        self.shared.frame.store(frame, Ordering::Relaxed);
    }

    /// Publish the JSON snapshot (AI calls this every ~1 s of game time).
    pub fn set_snapshot(&self, json: String) {
        *lock(&self.shared.snapshot) = Some(json);
    }

    /// Publish the board PNG (AI calls this every ~5 s of game time).
    pub fn set_board(&self, png: Vec<u8>) {
        *lock(&self.shared.board) = Some(png);
    }

    /// Write `status.json` + `board.png` into the writable data dir, so the
    /// state is inspectable even without a browser (every ~10 s of game time).
    pub fn write_files(&self) {
        let Some(dir) = &self.dir else { return };
        if let Some(s) = lock(&self.shared.snapshot).clone() {
            let _ = std::fs::write(format!("{dir}/status.json"), s);
        }
        if let Some(b) = lock(&self.shared.board).clone() {
            let _ = std::fs::write(format!("{dir}/board.png"), b);
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(s) = &mut self.server {
            s.stop();
        }
    }
}

impl Drop for Operator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ── board: the map as a PNG, drawn on a software canvas ───────────────────

/// Board picture width in pixels; the height follows the map's aspect.
pub const WIDTH: usize = 512;

/// The cached terrain layer (sampled once per run; terraforming is not
/// redrawn).
pub struct Terrain {
    pub w: usize,
    pub h: usize,
    rgb: Vec<[u8; 3]>,
}

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[u8; 3]>,
    /// Pixels per elmo.
    scale: f32,
}

impl Canvas {
    fn put(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.px[y as usize * self.w + x as usize] = c;
        }
    }

    #[allow(dead_code)]
    fn blend(&mut self, x: i32, y: i32, c: [u8; 3], alpha: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let p = &mut self.px[y as usize * self.w + x as usize];
            for i in 0..3 {
                p[i] = (p[i] as f32 * (1.0 - alpha) + c[i] as f32 * alpha).round() as u8;
            }
        }
    }

    fn at(&self, p: Vec3) -> (i32, i32) {
        ((p.x * self.scale).round() as i32, (p.z * self.scale).round() as i32)
    }

    fn disc(&mut self, x: i32, y: i32, r: i32, c: [u8; 3]) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.put(x + dx, y + dy, c);
                }
            }
        }
    }

    fn ring(&mut self, x: i32, y: i32, r: i32, c: [u8; 3]) {
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dy * dy;
                if d2 <= r * r && d2 >= (r - 1) * (r - 1) {
                    self.put(x + dx, y + dy, c);
                }
            }
        }
    }

    #[allow(dead_code)]
    fn faint_disc(&mut self, x: i32, y: i32, r: i32, c: [u8; 3], alpha: f32) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.blend(x + dx, y + dy, c, alpha);
                }
            }
        }
    }

    fn square(&mut self, x: i32, y: i32, r: i32, c: [u8; 3]) {
        for dy in -r..=r {
            for dx in -r..=r {
                self.put(x + dx, y + dy, c);
            }
        }
    }

    fn line(&mut self, (mut x0, mut y0): (i32, i32), (x1, y1): (i32, i32), c: [u8; 3]) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.put(x0, y0, c);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// A 3x5 glyph for digits and 'k'.
    fn glyph(&mut self, x: i32, y: i32, ch: char, c: [u8; 3]) {
        const DIGITS: [[u8; 5]; 10] = [
            [0b111, 0b101, 0b101, 0b101, 0b111],
            [0b010, 0b110, 0b010, 0b010, 0b111],
            [0b111, 0b001, 0b111, 0b100, 0b111],
            [0b111, 0b001, 0b111, 0b001, 0b111],
            [0b101, 0b101, 0b111, 0b001, 0b001],
            [0b111, 0b100, 0b111, 0b001, 0b111],
            [0b111, 0b100, 0b111, 0b101, 0b111],
            [0b111, 0b001, 0b010, 0b010, 0b010],
            [0b111, 0b101, 0b111, 0b101, 0b111],
            [0b111, 0b101, 0b111, 0b001, 0b111],
        ];
        const K: [u8; 5] = [0b100, 0b101, 0b110, 0b101, 0b101];
        let rows = match ch {
            '0'..='9' => DIGITS[ch as usize - '0' as usize],
            'k' => K,
            _ => return,
        };
        for (dy, row) in rows.iter().enumerate() {
            for dx in 0..3 {
                if row & (0b100 >> dx) != 0 {
                    self.put(x + dx, y + dy as i32, c);
                }
            }
        }
    }

    fn text(&mut self, x: i32, y: i32, s: &str, c: [u8; 3]) {
        for (i, ch) in s.chars().enumerate() {
            self.glyph(x + i as i32 * 4, y, ch, c);
        }
    }
}

/// Sample the terrain into a background layer: grey by height (water blue),
/// between the 5th and 95th percentile so a few peaks don't flatten the rest.
pub fn sample_terrain(map: Map) -> Terrain {
    let (mw, mh) = (map.width() as f32 * 8.0, map.height() as f32 * 8.0);
    let w = WIDTH;
    let h = ((w as f32 * mh / mw).round() as usize).clamp(64, 1024);
    let scale = w as f32 / mw;
    let mut sample: Vec<f32> = Vec::with_capacity(w * h / 64);
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            let e = map.elevation_at(x as f32 / scale, y as f32 / scale);
            if e >= 0.0 {
                sample.push(e);
            }
        }
    }
    sample.sort_by(|a, b| a.total_cmp(b));
    let pick = |q: f32| sample.get(((sample.len() as f32 - 1.0) * q) as usize).copied().unwrap_or(0.0);
    let (lo, hi) = (pick(0.05), pick(0.95));
    let span = (hi - lo).max(1.0);
    let mut rgb = vec![[0u8; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let e = map.elevation_at((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
            rgb[y * w + x] = if e < 0.0 {
                let d = ((-e) / 100.0).clamp(0.0, 1.0);
                [20, (60.0 - 30.0 * d) as u8, (110.0 - 40.0 * d) as u8]
            } else {
                // Quantized to 16 steps: flat areas compress to almost nothing.
                let g = (60.0 + 130.0 * ((e - lo) / span).clamp(0.0, 1.0)) as u8 / 8 * 8;
                [g, g, g.saturating_sub(10)]
            };
        }
    }
    Terrain { w, h, rgb }
}

pub struct BoardSpot {
    pub x: f32,
    pub z: f32,
    pub owned: bool,
    pub enemy: bool,
    pub blocked: bool,
}

pub enum UnitMark {
    Commander,
    Worker,
    Building,
    Combat,
}

pub struct BoardUnit {
    pub x: f32,
    pub z: f32,
    pub mark: UnitMark,
}

pub struct BoardEnemy {
    pub x: f32,
    pub z: f32,
    pub is_static: bool,
}

/// Plain-data view of the world for the board; built by the AI thread.
pub struct BoardState {
    /// Map width in elmos (the height follows from the terrain's aspect).
    pub map_w: f32,
    pub frame: i32,
    pub spots: Vec<BoardSpot>,
    pub geo: Vec<Vec3>,
    pub units: Vec<BoardUnit>,
    pub enemies: Vec<BoardEnemy>,
    pub our_start: Vec3,
    pub front: Vec3,
    pub enemy_target: Vec3,
}

/// Draw the board over the terrain; PNG bytes.
pub fn render_board(state: &BoardState, terrain: &Terrain) -> Vec<u8> {
    let mut c = Canvas { w: terrain.w, h: terrain.h, px: terrain.rgb.clone(), scale: terrain.w as f32 / state.map_w };
    let grid = [120u8, 120, 120];
    let label = [230u8, 230, 230];
    // Grid every 1000 elmos, labelled in thousands.
    let mut k = 1;
    while (k as f32 * 1000.0 * c.scale) < c.w as f32 {
        let x = (k as f32 * 1000.0 * c.scale).round() as i32;
        for y in 0..c.h as i32 {
            if y % 3 == 0 {
                c.put(x, y, grid);
            }
        }
        c.text(x + 2, 1, &format!("{k}k"), label);
        k += 1;
    }
    let mut k = 1;
    while (k as f32 * 1000.0 * c.scale) < c.h as f32 {
        let y = (k as f32 * 1000.0 * c.scale).round() as i32;
        for x in 0..c.w as i32 {
            if x % 3 == 0 {
                c.put(x, y, grid);
            }
        }
        c.text(1, y + 2, &format!("{k}k"), label);
        k += 1;
    }
    // Geothermal vents: small orange rings.
    for g in &state.geo {
        let (x, y) = c.at(*g);
        c.ring(x, y, 3, [255, 160, 40]);
    }
    // Metal spots: ours blue, enemy red, blocked grey, free white.
    for s in &state.spots {
        let (x, y) = c.at(Vec3::xz(s.x, s.z));
        let col = if s.owned {
            [80, 140, 255]
        } else if s.enemy {
            [255, 70, 70]
        } else if s.blocked {
            [90, 90, 90]
        } else {
            [235, 235, 235]
        };
        c.ring(x, y, 4, col);
    }
    // Front line: dim line our_start -> enemy_target, magenta rings at the front.
    c.line(c.at(state.our_start), c.at(state.enemy_target), [120, 50, 140]);
    let (fx, fy) = c.at(state.front);
    c.ring(fx, fy, 6, [255, 80, 255]);
    c.ring(fx, fy, 7, [255, 80, 255]);
    // Enemies in radar+LOS: red, statics as squares.
    for e in &state.enemies {
        let (x, y) = c.at(Vec3::xz(e.x, e.z));
        if e.is_static {
            c.square(x, y, 2, [255, 40, 40]);
        } else {
            c.disc(x, y, 1, [255, 40, 40]);
        }
    }
    // Our units.
    for u in &state.units {
        let (x, y) = c.at(Vec3::xz(u.x, u.z));
        match u.mark {
            UnitMark::Commander => c.disc(x, y, 3, [255, 230, 40]),
            UnitMark::Worker => c.disc(x, y, 2, [60, 230, 60]),
            UnitMark::Building => c.square(x, y, 1, [70, 110, 255]),
            UnitMark::Combat => c.disc(x, y, 1, [0, 255, 255]),
        }
    }
    // Frame counter, bottom-left.
    c.text(2, c.h as i32 - 7, &format!("{}", state.frame), label);
    encode(&c)
}

fn encode(c: &Canvas) -> Vec<u8> {
    use std::io::Cursor;
    let mut out = Vec::with_capacity(c.w * c.h / 4);
    {
        let mut enc = png::Encoder::new(Cursor::new(&mut out), c.w as u32, c.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::default());
        let mut w = match enc.write_header() {
            Ok(w) => w,
            Err(_) => return Vec::new(),
        };
        let flat: Vec<u8> = c.px.iter().flat_map(|p| p.iter().copied()).collect();
        if w.write_image_data(&flat).is_err() {
            return Vec::new();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_draws_and_encodes() {
        let mut c = Canvas { w: 64, h: 48, px: vec![[0; 3]; 64 * 48], scale: 0.01 };
        c.disc(10, 10, 3, [255, 0, 0]);
        c.ring(30, 30, 5, [0, 255, 0]);
        c.square(50, 10, 2, [0, 0, 255]);
        c.line((0, 0), (63, 47), [255, 255, 255]);
        c.text(2, 2, "12k", [200, 200, 200]);
        let png = encode(&c);
        assert_eq!(&png[1..4], b"PNG");
        assert!(png.len() < 64 * 48 * 3);
    }

    #[test]
    fn spec_parsing() {
        assert_eq!(parse_spec(None), None);
        assert_eq!(parse_spec(Some("".into())), None);
        assert_eq!(parse_spec(Some("off".into())), None);
        assert_eq!(parse_spec(Some("0".into())), None);
        assert_eq!(parse_spec(Some("on".into())), Some(format!("127.0.0.1:{DEFAULT_PORT}")));
        assert_eq!(parse_spec(Some("8055".into())), Some("127.0.0.1:8055".to_string()));
        assert_eq!(parse_spec(Some("0.0.0.0:9000".into())), Some("0.0.0.0:9000".to_string()));
    }
}
