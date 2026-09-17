use serde::{Deserialize, Serialize};
use std::sync::Mutex;

// macOS: daemon架构用18081端口probe; Windows/Linux: 自己spawn用1083端口
#[cfg(target_os = "macos")]
const SHELL_PORT: u16 = 18081;
#[cfg(not(target_os = "macos"))]
const SHELL_PORT: u16 = 1083;

#[cfg(target_os = "macos")]
const DAEMON_PAC_URL: &str = "http://127.0.0.1:18085/proxy.pac";

// ===== 代理接管安全（v1.2.1 移植自硅侣 tunnel.rs）=====
// 承诺：活着给用户完全无限制网络；任何退场（退出/崩溃/强杀）必恢复用户网络原样。
// 原值持久化到磁盘（原子写），崩溃后下次启动自愈恢复；无记录=不动用户设置。

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacServiceRecord {
    pub name: String,
    pub auto_enabled: bool,
    pub auto_url: String,
    pub socks_enabled: bool,
    pub socks_host: String,
    pub socks_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinProxyRecord {
    pub proxy_enable: String,
    pub proxy_server: String,
    pub proxy_override: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinuxProxyRecord {
    pub mode: String,
    pub socks_host: String,
    pub socks_port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProxyTakeoverRecord {
    version: u32,
    taken_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    macos: Option<Vec<MacServiceRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    windows: Option<WinProxyRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    linux: Option<LinuxProxyRecord>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 接管记录文件路径（与Tauri app_data_dir约定一致，跨平台）
fn takeover_record_path() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        std::path::PathBuf::from(
            shellexpand::tilde("~/Library/Application Support/com.xiaziteam.chatgpt-shell").to_string(),
        )
        .join("proxy_takeover.json")
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(base)
            .join("com.xiaziteam.chatgpt-shell")
            .join("proxy_takeover.json")
    }
    #[cfg(target_os = "linux")]
    {
        std::path::PathBuf::from(
            shellexpand::tilde("~/.config/com.xiaziteam.chatgpt-shell").to_string(),
        )
        .join("proxy_takeover.json")
    }
}

/// 原子写：tmp + rename，防写一半崩溃产生损坏记录
fn write_takeover_record(record: &ProxyTakeoverRecord) {
    let path = takeover_record_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(record) {
        Ok(json) => {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
                log_line("[takeover] 接管记录已持久化（原子写）");
            } else {
                log_line("[takeover] 记录写入失败");
            }
        }
        Err(e) => log_line(&format!("[takeover] 序列化失败: {}", e)),
    }
}

fn read_takeover_record() -> Option<ProxyTakeoverRecord> {
    let path = takeover_record_path();
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(r) => Some(r),
        Err(e) => {
            log_line(&format!("[takeover] 记录损坏({})，删除", e));
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

fn clear_takeover_record() {
    let _ = std::fs::remove_file(takeover_record_path());
}

#[cfg(target_os = "macos")]
fn snapshot_mac_services() -> Vec<MacServiceRecord> {
    get_all_network_services()
        .iter()
        .map(|svc| {
            let (s_enabled, s_host, s_port) =
                get_socks_proxy(svc).unwrap_or((false, String::new(), 0));
            MacServiceRecord {
                name: svc.clone(),
                auto_enabled: get_autoproxy_state(svc),
                auto_url: get_autoproxy_url(svc),
                socks_enabled: s_enabled,
                socks_host: s_host,
                socks_port: s_port,
            }
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn restore_mac_services(records: &[MacServiceRecord]) {
    for r in records {
        if r.socks_enabled && !r.socks_host.is_empty() {
            let _ = std::process::Command::new("networksetup")
                .args([
                    "-setsocksfirewallproxy",
                    &r.name,
                    &r.socks_host,
                    &r.socks_port.to_string(),
                ])
                .output();
            let _ = std::process::Command::new("networksetup")
                .args(["-setsocksfirewallproxystate", &r.name, "on"])
                .output();
        } else {
            let _ = std::process::Command::new("networksetup")
                .args(["-setsocksfirewallproxystate", &r.name, "off"])
                .output();
        }
        if r.auto_enabled && !r.auto_url.is_empty() {
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxyurl", &r.name, &r.auto_url])
                .output();
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxystate", &r.name, "on"])
                .output();
        } else {
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxystate", &r.name, "off"])
                .output();
        }
    }
}

#[cfg(target_os = "windows")]
fn snapshot_win_proxy() -> WinProxyRecord {
    WinProxyRecord {
        proxy_enable: win_reg_query("ProxyEnable").unwrap_or_else(|| "0x0".to_string()),
        proxy_server: win_reg_query("ProxyServer").unwrap_or_default(),
        proxy_override: win_reg_query("ProxyOverride").unwrap_or_default(),
    }
}

#[cfg(target_os = "windows")]
fn restore_win_proxy(r: &WinProxyRecord) {
    win_reg_set(
        "ProxyEnable",
        if r.proxy_enable.contains('1') { "1" } else { "0" },
        "REG_DWORD",
    );
    if r.proxy_server.is_empty() {
        win_reg_del("ProxyServer");
    } else {
        win_reg_set("ProxyServer", &r.proxy_server, "REG_SZ");
    }
    if r.proxy_override.is_empty() {
        win_reg_del("ProxyOverride");
    } else {
        win_reg_set("ProxyOverride", &r.proxy_override, "REG_SZ");
    }
    win_inet_refresh();
}

#[cfg(target_os = "linux")]
fn gsettings_get(schema: &str, key: &str) -> String {
    std::process::Command::new("gsettings")
        .args(["get", schema, key])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn snapshot_linux_proxy() -> LinuxProxyRecord {
    LinuxProxyRecord {
        mode: gsettings_get("org.gnome.system.proxy", "mode"),
        socks_host: gsettings_get("org.gnome.system.proxy.socks", "host"),
        socks_port: gsettings_get("org.gnome.system.proxy.socks", "port"),
    }
}

#[cfg(target_os = "linux")]
fn restore_linux_proxy(r: &LinuxProxyRecord) {
    let mode = if r.mode.is_empty() { "'none'" } else { &r.mode };
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.system.proxy", "mode", mode])
        .output();
    if !r.socks_host.is_empty() {
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.system.proxy.socks", "host", &r.socks_host])
            .output();
    }
    if !r.socks_port.is_empty() {
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.system.proxy.socks", "port", &r.socks_port])
            .output();
    }
}

fn debug_log_path() -> std::path::PathBuf {
    std::env::temp_dir().join("chatgpt-shell-debug.log")
}

pub fn log_line(msg: &str) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{}] {}\n", ts, msg);
    eprint!("{}", line);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log_path()) {
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn init_debug_log(version: &str) {
    let _ = std::fs::remove_file(debug_log_path());
    log_line(&format!("=== ChatGPT虾壳 v{} 启动 ===", version));
    log_line(&format!("cwd={:?} args={:?}", std::env::current_dir(), std::env::args().collect::<Vec<_>>()));
    if let Ok(exe) = std::env::current_exe() {
        log_line(&format!("exe={}", exe.display()));
    }
}

#[cfg(target_os = "macos")]
pub fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub server: String,
    pub server_port: u16,
    pub uuid: String,
    pub flow: String,
    pub server_name: String,
    pub public_key: String,
    pub short_id: String,
    pub route_domains: Vec<String>,
    pub plan: String,
}

pub struct TunnelManager {
    pub process: Mutex<Option<std::process::Child>>,
    pub status: Mutex<TunnelStatus>,
    pub config: Mutex<Option<TunnelConfig>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            process: Mutex::new(None),
            status: Mutex::new(TunnelStatus::Stopped),
            config: Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TunnelStatus {
    Stopped,
    Running { port: u16, mode: String },
}

pub fn validate_code_remote(code: &str) -> Result<TunnelConfig, String> {
    let url = "https://locatenotify.online/v1/code/validate";

    #[derive(serde::Serialize)]
    struct ValidateReq {
        code: String,
    }

    log_line(&format!("[validate] connecting to {}...", url));

    let resp: serde_json::Value = match ureq::post(url)
        .timeout(std::time::Duration::from_secs(15))
        .send_json(ValidateReq { code: code.to_string() })
    {
        Ok(r) => {
            log_line("[validate] got response, parsing json");
            r.into_json::<serde_json::Value>()
                .map_err(|e| format!("解析响应失败: {}", e))?
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            log_line(&format!("[validate] HTTP {} body: {}", code, body));
            return Err(format!("服务器返回错误 (HTTP {})", code));
        }
        Err(e) => {
            log_line(&format!("[validate] request failed: {:?}", e));
            return Err(format!("无法连接激活服务器: {}", e));
        }
    };

    log_line(&format!("[validate] parsed: {}", resp));

    if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let data = &resp["data"];
        let tunnel = data.get("tunnel");
        if tunnel.is_none() || tunnel.unwrap().is_null() {
            return Err("服务端未配置隧道凭证，请联系管理员".into());
        }
        let t = tunnel.unwrap();
        let plan = data["plan"].as_str().unwrap_or("basic").to_string();
        let route_domains: Vec<String> = t.get("route_domains")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        Ok(TunnelConfig {
            server: t["server"].as_str().unwrap_or("").to_string(),
            server_port: t["server_port"].as_u64().unwrap_or(443) as u16,
            uuid: t["uuid"].as_str().unwrap_or("").to_string(),
            flow: t["flow"].as_str().unwrap_or("xtls-rprx-vision").to_string(),
            server_name: t["server_name"].as_str().unwrap_or("www.cloudflare.com").to_string(),
            public_key: t["public_key"].as_str().unwrap_or("").to_string(),
            short_id: t["short_id"].as_str().unwrap_or("").to_string(),
            route_domains,
            plan,
        })
    } else {
        let msg = resp.get("message").and_then(|v| v.as_str()).unwrap_or("激活码无效");
        Err(msg.to_string())
    }
}

/// 启动自愈：上次崩溃/强杀未释放 → 从磁盘记录精确恢复；无记录 → 仅特征清理（不动用户设置）
pub fn recover_orphaned_takeover() {
    if read_takeover_record().is_some() {
        log_line("[takeover] 检测到孤儿接管记录（上次异常退场），自愈恢复中…");
        release_proxy();
    } else {
        fallback_feature_cleanup();
    }
}

/// 只释放不抛错 — 退出钩子/正常断开/崩溃自愈共用
/// 语义：有磁盘记录 → 精确恢复原值并删记录；无记录 → 不动用户设置，仅清理虾壳特征残留
pub fn release_proxy() {
    match read_takeover_record() {
        Some(record) => {
            #[cfg(target_os = "macos")]
            if let Some(ref mac) = record.macos {
                restore_mac_services(mac);
            }
            #[cfg(target_os = "windows")]
            if let Some(ref win) = record.windows {
                restore_win_proxy(win);
            }
            #[cfg(target_os = "linux")]
            if let Some(ref lin) = record.linux {
                restore_linux_proxy(lin);
            }
            clear_takeover_record();
            log_line("[takeover] 已按磁盘记录精确恢复用户网络原值");
        }
        None => {
            fallback_feature_cleanup();
            log_line("[takeover] 无接管记录，仅特征清理（用户设置未动）");
        }
    }
}

/// 特征清理：只清理确认为虾壳/硅侣留下的残留（老版本v1.2.0及更早兼容）
/// 无记录时不恢复任何"原值"——因为没有快照，宁可只清特征，不猜用户配置
fn fallback_feature_cleanup() {
    #[cfg(target_os = "windows")]
    {
        if let Some(server) = win_reg_query("ProxyServer") {
            if server.contains("127.0.0.1") && server.contains(&SHELL_PORT.to_string()) {
                log_line("[proxy] 发现残留SOCKS代理，清理");
                win_reg_set("ProxyEnable", "0", "REG_DWORD");
                win_inet_refresh();
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // 枚举所有network service，检查PAC是否包含chatgpt-shell或siliconmate-tunnel
        let services = get_all_network_services();
        for service in &services {
            if let Ok(output) = std::process::Command::new("networksetup")
                .args(["-getautoproxyurl", service])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                if text.contains("chatgpt-shell") || text.contains("siliconmate-tunnel") {
                    log_line(&format!("[proxy] Found leftover PAC proxy on '{}', cleaning up", service));
                    let _ = std::process::Command::new("networksetup")
                        .args(["-setautoproxystate", service, "off"])
                        .output();
                    let _ = std::process::Command::new("networksetup")
                        .args(["-setautoproxyurl", service, ""])
                        .output();
                }
            }
            // 也检查SOCKS残留
            if let Ok(output) = std::process::Command::new("networksetup")
                .args(["-getsocksfirewallproxy", service])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                if text.contains("127.0.0.1") && (text.contains("1083") || text.contains("18081")) {
                    log_line(&format!("[proxy] Found leftover SOCKS proxy on '{}', cleaning up", service));
                    let _ = std::process::Command::new("networksetup")
                        .args(["-setsocksfirewallproxystate", service, "off"])
                        .output();
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.system.proxy", "mode"])
            .output();
        if let Ok(o) = out {
            let text = String::from_utf8_lossy(&o.stdout);
            if text.contains("'manual'") {
                let socks_out = std::process::Command::new("gsettings")
                    .args(["get", "org.gnome.system.proxy.socks", "host"])
                    .output();
                if let Ok(so) = socks_out {
                    let st = String::from_utf8_lossy(&so.stdout);
                    if st.contains("127.0.0.1") {
                        log_line("[proxy] Found leftover Linux SOCKS proxy, cleaning up");
                        let _ = std::process::Command::new("gsettings")
                            .args(["set", "org.gnome.system.proxy", "mode", "none"])
                            .output();
                    }
                }
            }
        }
    }
}

fn find_singbox_binary() -> Option<String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            #[cfg(target_os = "windows")]
            {
                for name in ["sing-box.exe", "singbox.exe"] {
                    let bundled = dir.join(name);
                    if bundled.exists() {
                        return Some(bundled.to_string_lossy().to_string());
                    }
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                let bundled = dir.join("sing-box");
                if bundled.exists() {
                    return Some(bundled.to_string_lossy().to_string());
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        return Some("sing-box.exe".to_string());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let candidates = [
            "/usr/local/bin/sing-box",
            "$HOME/bin/sing-box",
            "$HOME/.local/bin/sing-box",
        ];
        for c in &candidates {
            let p = shellexpand::tilde(c).to_string();
            if std::path::Path::new(&p).exists() {
                return Some(p);
            }
        }
        None
    }
}

pub fn start_tunnel_internal(manager: &TunnelManager, config: &TunnelConfig) -> Result<u16, String> {
    // macOS: 统一daemon架构 — 不spawn sing-box，改为probe daemon端口
    #[cfg(target_os = "macos")]
    {
        use std::net::{TcpStream, SocketAddr, SocketAddrV4, Ipv4Addr};
        use std::time::Duration;

        let daemon_addr: SocketAddr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), SHELL_PORT).into();
        log_line(&format!("[tunnel] probing daemon at 127.0.0.1:{} ...", SHELL_PORT));

        match TcpStream::connect_timeout(&daemon_addr, Duration::from_secs(3)) {
            Ok(_) => {
                log_line(&format!("[tunnel] daemon is LISTEN on port {}", SHELL_PORT));
                let mode = if config.plan == "pro" { "全平台Pro" } else { "ChatGPT" };
                *manager.status.lock().unwrap() = TunnelStatus::Running { port: SHELL_PORT, mode: mode.to_string() };
                *manager.config.lock().unwrap() = Some(config.clone());
                Ok(SHELL_PORT)
            }
            Err(e) => {
                log_line(&format!("[tunnel] daemon NOT listening: {}", e));
                *manager.status.lock().unwrap() = TunnelStatus::Stopped;
                Err("隧道daemon未运行，请先启动统一隧道daemon".to_string())
            }
        }
    }

    // Windows/Linux: 自己spawn sing-box（无daemon架构）
    #[cfg(not(target_os = "macos"))]
    {
        let singbox = find_singbox_binary().ok_or("找不到sing-box")?;

        let domains_json: Vec<String> = config.route_domains.iter()
            .map(|d| format!("\"{}\"", d))
            .collect();
        let domains_str = domains_json.join(",");

        let route_config = if config.plan == "pro" {
            r#""rules": [
              { "ip_is_private": true, "outbound": "direct" },
              { "geoip": ["cn"], "outbound": "direct" },
              { "geosite": ["cn"], "outbound": "direct" }
            ],
            "final": "proxy""#.to_string()
        } else {
            format!(r#""rules": [
              {{
                "domain_suffix": [{}],
                "outbound": "proxy"
              }},
              {{ "ip_is_private": true, "outbound": "direct" }},
              {{ "geoip": ["cn"], "outbound": "direct" }},
              {{ "geosite": ["cn"], "outbound": "direct" }}
            ],
            "final": "direct""#, domains_str)
        };

        let geoip_path = std::env::temp_dir().join("chatgpt-shell-tunnel").join("geoip.db").to_string_lossy().to_string();
        let geosite_path = std::env::temp_dir().join("chatgpt-shell-tunnel").join("geosite.db").to_string_lossy().to_string();
        let singbox_config = format!(r#"{{
  "log": {{ "level": "warn" }},
  "inbounds": [{{
    "type": "socks",
    "tag": "socks-in",
    "listen": "127.0.0.1",
    "listen_port": {}
  }}],
  "outbounds": [
    {{
      "type": "vless",
      "tag": "proxy",
      "server": "{}",
      "server_port": {},
      "uuid": "{}",
      "flow": "{}",
      "tls": {{
        "enabled": true,
        "server_name": "{}",
        "utls": {{ "enabled": true, "fingerprint": "chrome" }},
        "reality": {{
          "enabled": true,
          "public_key": "{}",
          "short_id": "{}"
        }}
      }}
    }},
    {{ "type": "direct", "tag": "direct" }}
  ],
  "route": {{
    "geoip": {{ "path": "{}" }},
    "geosite": {{ "path": "{}" }},
    {}
  }}
}}"#, SHELL_PORT, config.server, config.server_port, config.uuid,
        config.flow, config.server_name, config.public_key, config.short_id,
        geoip_path, geosite_path, route_config);

        let config_dir = std::env::temp_dir().join("chatgpt-shell-tunnel");
        let _ = std::fs::create_dir_all(&config_dir);
        let config_path = config_dir.join("singbox-config.json");
        std::fs::write(&config_path, &singbox_config).map_err(|e| format!("写配置失败: {}", e))?;

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for db in ["geoip.db", "geosite.db"] {
                    let src = dir.join(db);
                    let dst = config_dir.join(db);
                    if src.exists() && !dst.exists() {
                        if std::fs::copy(&src, &dst).is_ok() {
                            log_line(&format!("[tunnel] 已复制 {} 到运行目录", db));
                        }
                    }
                }
            }
        }

        log_line(&format!("[tunnel] sing-box={} config={}", singbox, config_path.display()));
        log_line(&format!("[tunnel] server={}:{} uuid={}.. domains={}个",
            config.server, config.server_port, &config.uuid[..8], config.route_domains.len()));

        let child = std::process::Command::new(&singbox)
            .env("ENABLE_DEPRECATED_GEOIP", "true")
            .env("ENABLE_DEPRECATED_GEOSITE", "true")
            .args(["run", "-c", &config_path.to_string_lossy()])
            .spawn()
            .map_err(|e| format!("启动sing-box失败: {}", e))?;

        *manager.process.lock().unwrap() = Some(child);
        let mode = if config.plan == "pro" { "全平台Pro" } else { "ChatGPT" };
        *manager.status.lock().unwrap() = TunnelStatus::Running { port: SHELL_PORT, mode: mode.to_string() };
        *manager.config.lock().unwrap() = Some(config.clone());

        std::thread::sleep(std::time::Duration::from_secs(2));

        if let Some(ref mut child) = *manager.process.lock().unwrap() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    *manager.status.lock().unwrap() = TunnelStatus::Stopped;
                    *manager.config.lock().unwrap() = None;
                    return Err(format!("sing-box启动后立即退出: {}", status));
                }
                Ok(None) => {}
                Err(e) => return Err(format!("检查sing-box状态失败: {}", e)),
            }
        }

        Ok(SHELL_PORT)
    }
}

fn stop_tunnel_internal(manager: &TunnelManager) {
    // macOS: daemon是独立的，不kill进程，只清理状态
    #[cfg(target_os = "macos")]
    {
        *manager.status.lock().unwrap() = TunnelStatus::Stopped;
        *manager.config.lock().unwrap() = None;
        log_line("[tunnel] proxy disabled (daemon keeps running)");
    }

    // Windows/Linux: kill sing-box进程
    #[cfg(not(target_os = "macos"))]
    {
        let mut process = manager.process.lock().unwrap();
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *manager.status.lock().unwrap() = TunnelStatus::Stopped;
        *manager.config.lock().unwrap() = None;
        log_line("[tunnel] sing-box stopped");
    }
}

#[cfg(target_os = "windows")]
const WIN_INET_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

#[cfg(target_os = "windows")]
fn win_reg_query(value: &str) -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", WIN_INET_KEY, "/v", value])
        .output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with(value) {
            if let Some(v) = t.split_whitespace().last() {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn win_reg_set(value: &str, data: &str, reg_type: &str) {
    let _ = std::process::Command::new("reg")
        .args(["add", WIN_INET_KEY, "/v", value, "/t", reg_type, "/d", data, "/f"])
        .output();
}

#[cfg(target_os = "windows")]
fn win_reg_del(value: &str) {
    let _ = std::process::Command::new("reg")
        .args(["delete", WIN_INET_KEY, "/v", value, "/f"])
        .output();
}

#[cfg(target_os = "windows")]
fn win_inet_refresh() {
    let script = "Add-Type -Namespace Win32 -Name WinInet -MemberDefinition '[DllImport(\"wininet.dll\")] public static extern bool InternetSetOption(IntPtr h, int o, IntPtr b, int l);'; [Win32.WinInet]::InternetSetOption(0,39,0,0) | Out-Null; [Win32.WinInet]::InternetSetOption(0,37,0,0) | Out-Null";
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output();
}

pub fn set_system_proxy(enable: bool, socks_port: u16) {
    if enable {
        // ① 先快照原值并原子落盘（记录在，接管才发生——崩溃时也能自愈）
        #[cfg(target_os = "macos")]
        {
            let snapshot = snapshot_mac_services();
            write_takeover_record(&ProxyTakeoverRecord {
                version: 1,
                taken_at: now_secs(),
                macos: Some(snapshot),
                windows: None,
                linux: None,
            });
        }
        #[cfg(target_os = "windows")]
        {
            let snapshot = snapshot_win_proxy();
            write_takeover_record(&ProxyTakeoverRecord {
                version: 1,
                taken_at: now_secs(),
                macos: None,
                windows: Some(snapshot),
                linux: None,
            });
        }
        #[cfg(target_os = "linux")]
        {
            let snapshot = snapshot_linux_proxy();
            write_takeover_record(&ProxyTakeoverRecord {
                version: 1,
                taken_at: now_secs(),
                macos: None,
                windows: None,
                linux: Some(snapshot),
            });
        }

        // ② 应用接管
        apply_takeover(socks_port);
    } else {
        release_proxy();
    }
}

/// 接管系统代理（在各平台设置指向虾壳隧道的代理）
fn apply_takeover(socks_port: u16) {
    #[cfg(target_os = "windows")]
    {
        win_reg_set("ProxyEnable", "1", "REG_DWORD");
        win_reg_set(
            "ProxyServer",
            &format!("socks=127.0.0.1:{}", socks_port),
            "REG_SZ",
        );
        win_reg_set("ProxyOverride", "<local>", "REG_SZ");
        log_line(&format!("[proxy] Windows系统代理已开 socks=127.0.0.1:{}", socks_port));
        win_inet_refresh();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = socks_port; // macOS走daemon PAC，不直接用SOCKS端口
        let services = get_all_network_services();
        if services.is_empty() {
            log_line("[proxy] 无可用network service，接管未生效");
            return;
        }
        for svc in &services {
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxyurl", svc, DAEMON_PAC_URL])
                .output();
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxystate", svc, "on"])
                .output();
            // 关掉SOCKS手动代理，避免冲突（原值已在记录中，退场时恢复）
            let _ = std::process::Command::new("networksetup")
                .args(["-setsocksfirewallproxystate", svc, "off"])
                .output();
        }
        log_line(&format!(
            "[proxy] macOS PAC代理已开 url={} services={}个",
            DAEMON_PAC_URL,
            services.len()
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.system.proxy", "mode", "manual"])
            .output();
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.system.proxy.socks", "host", "127.0.0.1"])
            .output();
        let _ = std::process::Command::new("gsettings")
            .args([
                "set",
                "org.gnome.system.proxy.socks",
                "port",
                &socks_port.to_string(),
            ])
            .output();
        log_line(&format!("[proxy] Linux GNOME SOCKS代理已开 127.0.0.1:{}", socks_port));
    }
}

#[cfg(target_os = "macos")]
fn get_all_network_services() -> Vec<String> {
    let output = std::process::Command::new("networksetup")
        .args(["-listallnetworkservices"])
        .output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .skip(1) // 第一行是标题 "An asterisk (*) denotes..."
                .filter_map(|line| {
                    let trimmed = line.trim();
                    if !trimmed.starts_with('*') && !trimmed.is_empty() {
                        Some(trimmed.to_string())
                    } else {
                        None
                    }
                })
                .collect()
        }
        Err(e) => {
            log_line(&format!("[proxy] networksetup -listallnetworkservices failed: {}", e));
            Vec::new()
        }
    }
}

#[cfg(target_os = "macos")]
fn get_autoproxy_state(service: &str) -> bool {
    // 注意：networksetup 没有 -getautoproxystate 命令，
    // PAC启用状态要从 -getautoproxyurl 输出的 "Enabled: Yes/No" 解析
    let output = std::process::Command::new("networksetup")
        .args(["-getautoproxyurl", service])
        .output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .any(|l| l.starts_with("Enabled:") && l.contains("Yes"))
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "macos")]
fn get_autoproxy_url(service: &str) -> String {
    let output = std::process::Command::new("networksetup")
        .args(["-getautoproxyurl", service])
        .output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if line.starts_with("URL:") {
                    let url = line.trim_start_matches("URL:").trim().to_string();
                    // 未设置时 networksetup 输出 "URL: (null)"
                    return if url == "(null)" { String::new() } else { url };
                }
            }
            String::new()
        }
        Err(_) => String::new(),
    }
}

#[cfg(target_os = "macos")]
fn get_socks_proxy(service: &str) -> Option<(bool, String, u16)> {
    let output = std::process::Command::new("networksetup")
        .args(["-getsocksfirewallproxy", service])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut enabled = false;
    let mut host = String::new();
    let mut port: u16 = 0;
    for line in text.lines() {
        if line.starts_with("Enabled:") {
            enabled = line.contains("Yes");
        }
        if line.starts_with("Server:") {
            host = line.trim_start_matches("Server:").trim().to_string();
        }
        if line.starts_with("Port:") {
            port = line.trim_start_matches("Port:").trim().parse().unwrap_or(0);
        }
    }
    Some((enabled, host, port))
}

#[cfg(target_os = "macos")]
fn parse_dialog_text(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.contains("text returned:") {
            let parts: Vec<&str> = line.split("text returned:").collect();
            if parts.len() > 1 {
                return Some(parts[1].trim().trim_matches(',').to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub fn prompt_activation() -> Result<(String, TunnelConfig), String> {
    let script = r#"display dialog "ChatGPT虾壳 — 请输入激活码" default answer "" with title "ChatGPT虾壳" buttons {"取消", "激活"} default button 2 with icon note"#;
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("执行激活弹窗失败: {}", e))?;

    if !output.status.success() {
        return Err("用户取消".into());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let code = parse_dialog_text(&text).ok_or("无法解析激活码")?;

    if code.is_empty() {
        return Err("激活码不能为空".into());
    }

    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(r#"display notification "正在连接激活服务器…" with title "ChatGPT虾壳""#)
        .spawn();

    let tunnel_config = validate_code_remote(&code).map_err(|e| {
        log_line(&format!("[validate] FAILED: {}", e));
        let safe_detail = applescript_escape(&e);
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!("display dialog \"验证失败：{}\" buttons {{\"确定\"}} with title \"ChatGPT虾壳\" with icon stop", safe_detail))
            .output();
        e
    })?;
    log_line(&format!("[validate] OK plan={} domains={}个", tunnel_config.plan, tunnel_config.route_domains.len()));

    Ok((code, tunnel_config))
}

#[cfg(target_os = "windows")]
fn ps_escape(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(target_os = "windows")]
fn win_input_box(prompt: &str, title: &str) -> Option<String> {
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.Interaction]::InputBox('{}','{}','')",
        ps_escape(prompt), ps_escape(title));
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "windows")]
pub fn win_msgbox(text: &str, title: &str) {
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('{}','{}','OK','Error') | Out-Null",
        ps_escape(text), ps_escape(title));
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output();
}

#[cfg(target_os = "windows")]
pub fn prompt_activation() -> Result<(String, TunnelConfig), String> {
    let code = win_input_box("ChatGPT虾壳 — 请输入激活码", "ChatGPT虾壳").ok_or("用户取消")?;
    if code.is_empty() {
        return Err("用户取消".into());
    }
    let tunnel_config = validate_code_remote(&code).map_err(|e| {
        log_line(&format!("[validate] FAILED: {}", e));
        win_msgbox(&format!("验证失败：{}", e), "ChatGPT虾壳");
        e
    })?;
    log_line(&format!("[validate] OK plan={} domains={}个", tunnel_config.plan, tunnel_config.route_domains.len()));
    Ok((code, tunnel_config))
}

#[cfg(target_os = "linux")]
pub fn prompt_activation() -> Result<(String, TunnelConfig), String> {
    let output = std::process::Command::new("zenity")
        .args([
            "--entry",
            "--title=ChatGPT虾壳",
            "--text=请输入激活码",
            "--hide-text",
            "--width=400",
        ])
        .output()
        .map_err(|e| format!("执行zenity弹窗失败(请安装zenity): {}", e))?;

    if !output.status.success() {
        return Err("用户取消".into());
    }

    let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if code.is_empty() {
        return Err("激活码不能为空".into());
    }

    let tunnel_config = validate_code_remote(&code).map_err(|e| {
        log_line(&format!("[validate] FAILED: {}", e));
        let _ = std::process::Command::new("zenity")
            .args(["--error", &format!("--text=验证失败：{}", e.replace('"', "'")), "--title=ChatGPT虾壳", "--width=400"])
            .output();
        e
    })?;
    log_line(&format!("[validate] OK plan={} domains={}个", tunnel_config.plan, tunnel_config.route_domains.len()));
    Ok((code, tunnel_config))
}

#[tauri::command]
pub fn login_and_connect(
    manager: tauri::State<'_, TunnelManager>,
    _account: String,
    _password: String,
) -> Result<String, String> {
    let config = manager.config.lock().unwrap().clone()
        .ok_or("未激活")?;

    let port = start_tunnel_internal(&manager, &config)?;
    set_system_proxy(true, port);

    #[cfg(target_os = "macos")]
    {
        let url = if config.plan == "pro" {
            "https://www.google.com/"
        } else {
            "https://chatgpt.com/"
        };
        let _ = std::process::Command::new("open")
            .args(["-a", "Safari", url])
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let url = if config.plan == "pro" {
            "https://www.google.com/"
        } else {
            "https://chatgpt.com/"
        };
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let url = if config.plan == "pro" {
            "https://www.google.com/"
        } else {
            "https://chatgpt.com/"
        };
        let _ = std::process::Command::new("xdg-open")
            .arg(url)
            .spawn();
    }

    let mode = if config.plan == "pro" { "全平台Pro模式" } else { "ChatGPT模式" };
    #[cfg(target_os = "macos")]
    return Ok(format!("隧道已启动 · {} · Safari已打开", mode));
    #[cfg(target_os = "linux")]
    return Ok(format!("隧道已启动 · {} · 浏览器已打开", mode));
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return Ok(format!("隧道已启动 · {} · 浏览器已打开", mode));
}

#[tauri::command]
pub fn close_chatgpt(manager: tauri::State<'_, TunnelManager>) -> Result<String, String> {
    set_system_proxy(false, SHELL_PORT);
    stop_tunnel_internal(&manager);
    Ok("已断开连接 · 代理已恢复".into())
}

#[tauri::command]
pub fn tunnel_status(manager: tauri::State<'_, TunnelManager>) -> TunnelStatus {
    manager.status.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 真实系统测试共享系统设置与记录文件，必须串行
    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ===== 接管记录纯IO往返（不动系统设置）=====
    #[test]
    fn test_takeover_record_roundtrip() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let record = ProxyTakeoverRecord {
            version: 1,
            taken_at: 42,
            macos: Some(vec![MacServiceRecord {
                name: "Wi-Fi".into(),
                auto_enabled: true,
                auto_url: "http://127.0.0.1:1/user.pac".into(),
                socks_enabled: false,
                socks_host: String::new(),
                socks_port: 0,
            }]),
            windows: None,
            linux: None,
        };
        write_takeover_record(&record);
        let back = read_takeover_record().expect("记录应可读回");
        assert_eq!(back.version, 1);
        assert_eq!(back.taken_at, 42);
        let mac = back.macos.expect("macos段存在");
        assert_eq!(mac[0].name, "Wi-Fi");
        assert_eq!(mac[0].auto_url, "http://127.0.0.1:1/user.pac");
        clear_takeover_record();
        assert!(read_takeover_record().is_none(), "清除后记录应为空");
    }

    // ===== 损坏记录自清理 =====
    #[test]
    fn test_corrupted_record_self_heals() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let path = takeover_record_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, "{not valid json!!!");
        assert!(read_takeover_record().is_none(), "损坏记录应返回None并自删");
        assert!(!path.exists(), "损坏记录文件应被删除");
    }

    // ===== 真实系统接管/恢复（macOS，带备份守卫）=====
    // 模拟用户自有PAC → 接管 → 断开恢复 → 应精确还原用户值
    #[cfg(target_os = "macos")]
    #[test]
    fn test_takeover_release_real() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let services = get_all_network_services();
        if services.is_empty() {
            panic!("无network service可测试");
        }
        let svc = &services[0];

        // 备份守卫：记录真实原值，测试任何路径失败都还原
        let backup_enabled = get_autoproxy_state(svc);
        let backup_url = get_autoproxy_url(svc);
        let (backup_socks_on, backup_socks_host, backup_socks_port) =
            get_socks_proxy(svc).unwrap_or((false, String::new(), 0));

        let restore_backup = |ctx: &str| {
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxyurl", svc, &backup_url])
                .output();
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxystate", svc, if backup_enabled { "on" } else { "off" }])
                .output();
            if backup_socks_on && !backup_socks_host.is_empty() {
                let _ = std::process::Command::new("networksetup")
                    .args([
                        "-setsocksfirewallproxy",
                        svc,
                        &backup_socks_host,
                        &backup_socks_port.to_string(),
                    ])
                    .output();
                let _ = std::process::Command::new("networksetup")
                    .args(["-setsocksfirewallproxystate", svc, "on"])
                    .output();
            } else {
                let _ = std::process::Command::new("networksetup")
                    .args(["-setsocksfirewallproxystate", svc, "off"])
                    .output();
            }
            eprintln!("[test] 备份已还原({})", ctx);
        };

        // 1. 模拟用户自有PAC
        let user_url = "http://127.0.0.1:65534/user-own.pac";
        let _ = std::process::Command::new("networksetup")
            .args(["-setautoproxyurl", svc, user_url])
            .output();
        let _ = std::process::Command::new("networksetup")
            .args(["-setautoproxystate", svc, "on"])
            .output();

        // 2. 接管
        set_system_proxy(true, SHELL_PORT);
        let rec_exists = read_takeover_record().is_some();
        let now_url = get_autoproxy_url(svc);
        let now_on = get_autoproxy_state(svc);
        if !rec_exists || now_url != DAEMON_PAC_URL || !now_on {
            restore_backup("接管断言失败");
            panic!(
                "接管失败: record={} url={} on={}",
                rec_exists, now_url, now_on
            );
        }

        // 3. 断开 → 应精确恢复用户值
        set_system_proxy(false, SHELL_PORT);
        let restored_url = get_autoproxy_url(svc);
        let restored_on = get_autoproxy_state(svc);
        let rec_gone = read_takeover_record().is_none();
        let ok = rec_gone && restored_url == user_url && restored_on;
        restore_backup("正常收尾");
        assert!(ok, "恢复失败: rec_gone={} url={} on={}", rec_gone, restored_url, restored_on);
    }

    // ===== 崩溃自愈：孤儿记录在下次启动时恢复 =====
    #[cfg(target_os = "macos")]
    #[test]
    fn test_crash_self_heal_real() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let services = get_all_network_services();
        if services.is_empty() {
            panic!("无network service可测试");
        }
        let svc = &services[0];

        let backup_enabled = get_autoproxy_state(svc);
        let backup_url = get_autoproxy_url(svc);
        let restore_backup = || {
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxyurl", svc, &backup_url])
                .output();
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxystate", svc, if backup_enabled { "on" } else { "off" }])
                .output();
        };

        // 模拟用户自有PAC → 接管 → 崩溃（不调release）→ 下次启动自愈
        let user_url = "http://127.0.0.1:65534/crash-test.pac";
        let _ = std::process::Command::new("networksetup")
            .args(["-setautoproxyurl", svc, user_url])
            .output();
        let _ = std::process::Command::new("networksetup")
            .args(["-setautoproxystate", svc, "on"])
            .output();

        set_system_proxy(true, SHELL_PORT); // 接管
        // —— 模拟崩溃：进程直接消失，什么都不做 ——

        let rec_exists = read_takeover_record().is_some();
        if !rec_exists {
            restore_backup();
            panic!("崩溃场景记录应仍在磁盘");
        }
        recover_orphaned_takeover(); // 下次启动自愈
        let healed_url = get_autoproxy_url(svc);
        let healed_on = get_autoproxy_state(svc);
        let rec_gone = read_takeover_record().is_none();
        let ok = rec_gone && healed_url == user_url && healed_on;
        restore_backup();
        assert!(ok, "自愈失败: rec_gone={} url={} on={}", rec_gone, healed_url, healed_on);
    }
}
