#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tunnel;

struct ActivationInfo {
    code: String,
    mode: String,
}

fn main() {
    tunnel::init_debug_log(env!("CARGO_PKG_VERSION"));
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        tunnel::cleanup_leftover_proxy();
    }

    let (code, tunnel_config) = match std::env::var("SHELL_TEST_CODE") {
        Ok(c) if !c.trim().is_empty() => {
            let c = c.trim().to_string();
            tunnel::log_line("[main] SHELL_TEST_CODE 模式，跳过弹窗");
            match tunnel::validate_code_remote(&c) {
                Ok(t) => (c, t),
                Err(e) => {
                    tunnel::log_line(&format!("[main] SHELL_TEST_CODE 验证失败: {}", e));
                    std::process::exit(1);
                }
            }
        }
        _ => match tunnel::prompt_activation() {
            Ok(creds) => creds,
            Err(e) => {
                tunnel::log_line(&format!("[main] 激活流程退出: {}", e));
                std::process::exit(0);
            }
        },
    };
    tunnel::log_line(&format!("[main] 激活成功 plan={} code={}…",
        tunnel_config.plan, &code[..4.min(code.len())]));

    let mode = if tunnel_config.plan == "pro" { "全平台Pro" } else { "ChatGPT" };

    let mgr = tunnel::TunnelManager::new();

    let start_result = tunnel::start_tunnel_internal(&mgr, &tunnel_config);
    match &start_result {
        Ok(port) => {
            tunnel::set_system_proxy(true, *port);
            #[cfg(target_os = "macos")]
            {
                let url = if tunnel_config.plan == "pro" {
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
                let url = if tunnel_config.plan == "pro" {
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
                let url = if tunnel_config.plan == "pro" {
                    "https://www.google.com/"
                } else {
                    "https://chatgpt.com/"
                };
                let _ = std::process::Command::new("xdg-open")
                    .arg(url)
                    .spawn();
            }
        }
        Err(e) => {
            tunnel::log_line(&format!("[autostart] failed: {}", e));
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .arg("-e")
                    .arg(format!("display dialog \"启动隧道失败：{}\" buttons {{\"确定\"}} with title \"ChatGPT虾壳\" with icon stop", tunnel::applescript_escape(&e)))
                    .output();
            }
            #[cfg(target_os = "windows")]
            {
                tunnel::win_msgbox(&format!("启动隧道失败：{}", e), "ChatGPT虾壳");
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("zenity")
                    .args(["--error", &format!("--text=启动隧道失败：{}", e), "--title=ChatGPT虾壳", "--width=400"])
                    .output();
            }
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(mgr)
        .manage(ActivationInfo {
            code,
            mode: mode.to_string(),
        })
        .manage(start_result.is_ok())
        .invoke_handler(tauri::generate_handler![
            tunnel::login_and_connect,
            tunnel::close_chatgpt,
            tunnel::tunnel_status,
            get_activation_info,
        ])
        .setup(|app| {
            let _ = app;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running chatgpt-shell");
}

#[tauri::command]
fn get_activation_info(info: tauri::State<'_, ActivationInfo>) -> String {
    format!("{}模式 · 激活码: {}...", info.mode, &info.code[..4.min(info.code.len())])
}
