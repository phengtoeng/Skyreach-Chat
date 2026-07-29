//! Live status/analytics service for the Skyreach seal network + the WCAHT chain it runs on.
//!
//! Aggregates (server-side, so the browser needs no CORS and no chain API key):
//!   * per-node relay message counters (`/stats` on each relay) — deduped to a cluster figure,
//!   * relay/gateway/directory liveness for each node,
//!   * live WCAHT chain metrics from a validator `/health` (slot, finalized slot, block height).
//! and serves:
//!   GET /api  → the aggregated JSON snapshot (refreshed on a background tick)
//!   GET /     → a self-contained animated dashboard that polls /api
//!
//! Env: `WCAHT_STATS_NODES` (comma IPs, default N5,N6,N7), `WCAHT_STATS_CHAIN`
//! (validator health URL, default http://127.0.0.1:8901/health).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// Run the status service (blocking) on `addr`, e.g. `"0.0.0.0:9300"`.
pub fn serve_stats(addr: &str) -> Result<()> {
    let nodes: Vec<String> = std::env::var("WCAHT_STATS_NODES")
        .unwrap_or_else(|_| "139.99.150.23,51.79.176.134,51.79.162.80".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let chain_url =
        std::env::var("WCAHT_STATS_CHAIN").unwrap_or_else(|_| "http://127.0.0.1:8901/health".to_string());

    let snapshot = Arc::new(Mutex::new(json!({ "ready": false })));

    // background refresher: rebuild the aggregate every 2s so /api is a cheap cache read.
    {
        let snapshot = snapshot.clone();
        let nodes = nodes.clone();
        std::thread::spawn(move || {
            let http = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .expect("http");
            loop {
                let snap = build_snapshot(&http, &nodes, &chain_url);
                *snapshot.lock().unwrap() = snap;
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("stats bind {addr}: {e}"))?;
    println!("skyreach status dashboard on http://{addr}  (nodes: {})", nodes.join(", "));
    for req in server.incoming_requests() {
        let (code, ctype, body): (u16, &str, String) = match req.url() {
            "/api" | "/api/" => (200, "application/json", snapshot.lock().unwrap().to_string()),
            "/" | "/index.html" => (200, "text/html; charset=utf-8", DASHBOARD_HTML.to_string()),
            _ => (404, "application/json", r#"{"error":"not found"}"#.to_string()),
        };
        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap();
        let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code).with_header(header));
    }
    Ok(())
}

fn get_json(http: &reqwest::blocking::Client, url: &str) -> Option<Value> {
    http.get(url).send().ok()?.json().ok()
}
fn http_code(http: &reqwest::blocking::Client, url: &str) -> u16 {
    http.get(url).send().map(|r| r.status().as_u16()).unwrap_or(0)
}

fn build_snapshot(http: &reqwest::blocking::Client, nodes: &[String], chain_url: &str) -> Value {
    let mut node_view = Vec::new();
    let mut cluster_messages = 0u64;
    let mut cluster_mailboxes = 0u64;
    let mut per_minute: std::collections::BTreeMap<i64, u64> = std::collections::BTreeMap::new();
    let mut services_up = 0u32;

    for (i, ip) in nodes.iter().enumerate() {
        let name = ["N5", "N6", "N7"].get(i).copied().unwrap_or("N?");
        let stats = get_json(http, &format!("http://{ip}:9200/stats"));
        let relay_up = stats.is_some();
        // gateway healthy = 425 (locked-until-finalise); directory healthy = 404 (empty lookup).
        let gw_up = http_code(http, &format!("http://{ip}:9201/release/x")) == 425;
        let dir_up = http_code(http, &format!("http://{ip}:9988/lookup/x")) == 404;
        services_up += relay_up as u32 + gw_up as u32 + dir_up as u32;

        if let Some(s) = &stats {
            let nt = s.get("node_total").and_then(Value::as_u64).unwrap_or(0);
            let mb = s.get("mailboxes").and_then(Value::as_u64).unwrap_or(0);
            cluster_messages = cluster_messages.max(nt); // gossip replicates → dedup by max
            cluster_mailboxes = cluster_mailboxes.max(mb);
            if let Some(pm) = s.get("per_minute").and_then(Value::as_array) {
                for b in pm {
                    let t = b.get("t").and_then(Value::as_i64).unwrap_or(0);
                    let n = b.get("n").and_then(Value::as_u64).unwrap_or(0);
                    let e = per_minute.entry(t).or_default();
                    *e = (*e).max(n);
                }
            }
        }
        node_view.push(json!({ "name": name, "ip": ip, "relay": relay_up, "gateway": gw_up, "directory": dir_up }));
    }

    let pm: Vec<Value> = per_minute.iter().map(|(t, n)| json!({ "t": t, "n": n })).collect();

    // live chain metrics from a validator /health (server-side fetch: no CORS, no API key).
    let chain = match get_json(http, chain_url) {
        Some(h) => {
            let slot = h.get("slot").and_then(Value::as_i64).unwrap_or(0);
            let fin = h.get("finalized_slot").and_then(Value::as_i64).unwrap_or(0);
            json!({
                "up": true,
                "slot": slot,
                "finalized_slot": fin,
                "lag": (slot - fin).max(0),
                "block_height": h.get("block_height").and_then(Value::as_i64).unwrap_or(0),
                "last_block_slot": h.get("last_block_slot").and_then(Value::as_i64).unwrap_or(0),
                "status": h.get("status").and_then(Value::as_str).unwrap_or("?"),
                "peers": h.get("peers").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0),
                "chain_id": 7789,
                "slot_ms": 400,
            })
        }
        None => json!({ "up": false }),
    };

    json!({
        "ready": true,
        "messages": cluster_messages,
        "mailboxes": cluster_mailboxes,
        "per_minute": pm,
        "nodes": node_view,
        "services_up": services_up,
        "services_total": nodes.len() * 3,
        "chain": chain,
        "updated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    })
}

/// Self-contained animated dashboard (no external assets). Polls /api and renders live.
const DASHBOARD_HTML: &str = r##"<!doctype html><html lang="en"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Skyreach · live on WCAHT</title>
<style>
:root{--bg:#070b16;--card:#0e1526;--line:#1c2740;--ink:#e8eefc;--sub:#7c8bb0;--accent:#3aa0ff;--good:#2ee6a6;--warn:#ffb03a;--bad:#ff5470}
*{box-sizing:border-box;margin:0;padding:0}
body{background:radial-gradient(1200px 600px at 70% -10%,#122036,transparent),var(--bg);color:var(--ink);font:15px/1.4 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;-webkit-font-smoothing:antialiased;min-height:100vh}
.wrap{max-width:1040px;margin:0 auto;padding:28px 20px 60px}
.top{display:flex;align-items:center;gap:12px;margin-bottom:22px;flex-wrap:wrap}
.logo{width:34px;height:34px;border-radius:9px;background:linear-gradient(135deg,#3aa0ff,#2ee6a6);display:flex;align-items:center;justify-content:center;font-size:18px}
h1{font-size:20px;font-weight:700;letter-spacing:.2px}
.sub{color:var(--sub);font-size:13px}
.live{margin-left:auto;display:flex;align-items:center;gap:8px;font-size:12px;color:var(--good);font-weight:600;text-transform:uppercase;letter-spacing:.08em}
.dot{width:9px;height:9px;border-radius:50%;background:var(--good);box-shadow:0 0 0 0 rgba(46,230,166,.6);animation:pulse 1.8s infinite}
@keyframes pulse{0%{box-shadow:0 0 0 0 rgba(46,230,166,.55)}70%{box-shadow:0 0 0 10px rgba(46,230,166,0)}100%{box-shadow:0 0 0 0 rgba(46,230,166,0)}}
.grid{display:grid;grid-template-columns:repeat(4,1fr);gap:14px;margin-bottom:14px}
@media(max-width:820px){.grid{grid-template-columns:repeat(2,1fr)}}
.card{background:linear-gradient(180deg,rgba(255,255,255,.02),transparent),var(--card);border:1px solid var(--line);border-radius:16px;padding:16px 18px}
.k{color:var(--sub);font-size:12px;text-transform:uppercase;letter-spacing:.07em;margin-bottom:8px}
.v{font-size:30px;font-weight:800;font-variant-numeric:tabular-nums;letter-spacing:-.5px}
.v small{font-size:13px;font-weight:600;color:var(--sub);letter-spacing:0}
.accent{color:var(--accent)} .green{color:var(--good)}
.big{grid-column:span 2}
.chart-card{background:var(--card);border:1px solid var(--line);border-radius:16px;padding:16px 18px;margin-bottom:14px}
.chart-head{display:flex;align-items:baseline;gap:10px;margin-bottom:10px}
canvas{width:100%;height:150px;display:block}
.nodes{display:grid;grid-template-columns:repeat(3,1fr);gap:14px}
@media(max-width:820px){.nodes{grid-template-columns:1fr}}
.node{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:14px 16px}
.node h3{font-size:14px;margin-bottom:10px;display:flex;justify-content:space-between}
.svc{display:flex;align-items:center;gap:8px;color:var(--sub);font-size:13px;padding:3px 0}
.pill{width:8px;height:8px;border-radius:50%;background:var(--bad)}
.pill.on{background:var(--good);box-shadow:0 0 8px rgba(46,230,166,.6)}
.foot{color:var(--sub);font-size:12px;margin-top:18px;display:flex;gap:16px;flex-wrap:wrap}
.mono{font-variant-numeric:tabular-nums}
</style></head><body><div class="wrap">
<div class="top">
  <div class="logo">🛡️</div>
  <div><h1>Skyreach</h1><div class="sub">sealed messenger — running live on the WCAHT chain</div></div>
  <div class="live"><span class="dot"></span> Live</div>
</div>

<div class="grid">
  <div class="card"><div class="k">Sealed messages delivered</div><div class="v green" id="messages">–</div></div>
  <div class="card"><div class="k">WCAHT slot</div><div class="v accent mono" id="slot">–</div></div>
  <div class="card"><div class="k">Finalized slot</div><div class="v mono" id="finalized">–</div></div>
  <div class="card"><div class="k">Block height</div><div class="v mono" id="height">–</div></div>
</div>

<div class="chart-card">
  <div class="chart-head"><div class="k" style="margin:0">Messages / minute (last hour)</div><div class="sub" id="ratenote"></div></div>
  <canvas id="chart"></canvas>
</div>

<div class="nodes" id="nodes"></div>

<div class="foot">
  <span>chain id <b class="mono">7789</b></span>
  <span>slot cadence <b class="mono">400ms</b></span>
  <span id="services">–</span>
  <span id="finlag"></span>
  <span id="updated"></span>
</div>
</div>
<script>
let slot=0, slotTarget=0, msgShown=0, msgTarget=0, series=[], lastUpdate=0;
function fmt(n){return (n||0).toLocaleString('en-US')}
async function poll(){
  try{
    const r=await fetch('/api',{cache:'no-store'}); const d=await r.json();
    if(!d.ready) return;
    msgTarget=d.messages||0;
    if(d.chain&&d.chain.up){
      slotTarget=d.chain.slot; if(slot===0) slot=slotTarget;
      document.getElementById('finalized').textContent=fmt(d.chain.finalized_slot);
      document.getElementById('height').textContent=fmt(d.chain.block_height);
      document.getElementById('finlag').innerHTML='finality lag <b class="mono">'+d.chain.lag+'</b> slots (~'+(d.chain.lag*0.4).toFixed(1)+'s)';
    }
    series=(d.per_minute||[]).map(b=>b.n);
    const recent=series.slice(-10).reduce((a,b)=>a+b,0);
    document.getElementById('ratenote').textContent=recent+' in the last 10 min';
    document.getElementById('services').innerHTML='services <b class="mono">'+d.services_up+'/'+d.services_total+'</b> healthy';
    lastUpdate=d.updated;
    renderNodes(d.nodes||[]);
    draw();
  }catch(e){}
}
function renderNodes(ns){
  const box=document.getElementById('nodes');
  box.innerHTML=ns.map(n=>{
    const s=(lbl,on)=>'<div class="svc"><span class="pill'+(on?' on':'')+'"></span>'+lbl+'</div>';
    const up=(n.relay?1:0)+(n.gateway?1:0)+(n.directory?1:0);
    return '<div class="node"><h3><span>'+n.name+'</span><span class="sub mono">'+up+'/3</span></h3>'+
      s('relay',n.relay)+s('gateway',n.gateway)+s('directory',n.directory)+'</div>';
  }).join('');
}
function draw(){
  const c=document.getElementById('chart'), dpr=window.devicePixelRatio||1;
  const w=c.clientWidth, h=c.clientHeight; c.width=w*dpr; c.height=h*dpr;
  const x=c.getContext('2d'); x.scale(dpr,dpr); x.clearRect(0,0,w,h);
  const data=series.length?series:[0]; const max=Math.max(1,...data);
  const n=data.length, pad=6, bw=(w-pad*2)/n;
  // gridlines
  x.strokeStyle='rgba(255,255,255,.05)'; x.lineWidth=1;
  for(let i=0;i<=3;i++){const gy=pad+(h-pad*2)*i/3; x.beginPath(); x.moveTo(0,gy); x.lineTo(w,gy); x.stroke();}
  // area + line
  const pts=data.map((v,i)=>[pad+bw*i+bw/2,(h-pad)-(h-pad*2)*(v/max)]);
  const grad=x.createLinearGradient(0,0,0,h); grad.addColorStop(0,'rgba(58,160,255,.35)'); grad.addColorStop(1,'rgba(58,160,255,0)');
  x.beginPath(); x.moveTo(pts[0][0],h-pad); pts.forEach(p=>x.lineTo(p[0],p[1])); x.lineTo(pts[n-1][0],h-pad); x.closePath(); x.fillStyle=grad; x.fill();
  x.beginPath(); pts.forEach((p,i)=>i?x.lineTo(p[0],p[1]):x.moveTo(p[0],p[1])); x.strokeStyle='#3aa0ff'; x.lineWidth=2; x.stroke();
  const last=pts[n-1]; x.beginPath(); x.arc(last[0],last[1],3.5,0,7); x.fillStyle='#2ee6a6'; x.fill();
}
// smooth ticking between polls
setInterval(()=>{
  if(slotTarget>slot) slot=Math.min(slotTarget,slot+Math.ceil((slotTarget-slot)/6)+1);
  document.getElementById('slot').textContent=fmt(slot);
  if(msgTarget!==msgShown){const step=Math.max(1,Math.ceil(Math.abs(msgTarget-msgShown)/8)); msgShown+=Math.sign(msgTarget-msgShown)*step; if(Math.abs(msgTarget-msgShown)<step)msgShown=msgTarget; document.getElementById('messages').textContent=fmt(msgShown);}
  if(lastUpdate){const ago=Math.max(0,Math.floor(Date.now()/1000)-lastUpdate); document.getElementById('updated').textContent='updated '+ago+'s ago';}
},400);
poll(); setInterval(poll,2000); window.addEventListener('resize',draw);
</script></body></html>"##;
