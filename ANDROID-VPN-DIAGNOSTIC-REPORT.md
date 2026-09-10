# ChatGPT虾壳 Android版 VPN路由诊断报告

**日期**: 2026-09-09
**版本**: d5299ef (v1.0.1 + setBlocking修复)
**症状**: VPN开启后国内全断（微信/国内网站不可用），与上一版完全一致

---

## 一、根因分析

### 致命缺陷 #1：缺少 `auto_detect_interface`（流量环路）

**当前配置**:
```json
"route": {
  "geoip": {"path": "..."},
  "geosite": {"path": "..."},
  "rules": [...],
  "final": "direct"
}
```

**问题**: 当 `auto_route: true` 时，sing-box官方文档**强制要求**设置 `route.auto_detect_interface` 或 `route.default_interface`，否则 direct 出站的 socket 不绑定物理网卡，流量会**回环进入TUN**：

```
App请求 → TUN → sing-box → direct出站 → 新socket → 被VPN路由捕获 → TUN → 死循环！
```

**官方原文**: "To avoid traffic loopback, set `route.auto_detect_interface` or `route.default_interface` or `outbound.bind_interface`"

**修复**:
```json
"route": {
  "auto_detect_interface": true,
  ...
}
```

这是**国内全断的根本原因**：所有匹配 direct 的流量（微信、CN站点）都回环了，永远到不了物理网卡。

---

### 致命缺陷 #2：DNS劫持方式错误

**当前配置**:
```json
{"protocol": "dns", "outbound": "dns-out"}
```

**问题**: `outbound: dns-out` 只是把DNS包路由到 dns 出站，但在TUN模式下，DNS查询发往 TUN 的 DNS 地址（172.19.0.2:53），这种路由方式无法正确拦截并交给 sing-box DNS 模块处理。

**正确做法**（sing-box 1.11+ TUN模式必需）:
```json
{"protocol": "dns", "action": "hijack-dns"}
```

`hijack-dns` 是专门为TUN模式设计的动作，将DNS查询正确劫持到 sing-box 的DNS模块，然后按 dns.rules 分流（CN→223.5.5.5，外网→1.1.1.1）。

---

### 严重缺陷 #3：`stack: "system"` 在Android上不可靠

**问题**: `system` 栈使用系统内核网络栈，在部分Android ROM上UDP处理有bug，导致DNS查询（UDP 53）丢包或超时。

**修复**: 改为 `"stack": "mixed"`（系统TCP + gVisor UDP），这是Android上最稳定的组合：
- TCP走系统栈 → 稳定可靠
- UDP走gVisor用户态栈 → 绕开Android内核UDP bug

| 栈模式 | Android表现 |
|--------|------------|
| system | UDP在部分ROM丢包 |
| gvisor | 内存高，微信通话崩溃(#2740) |
| **mixed** | **TCP系统+UDP用户态，最稳定** |

---

### 缺陷 #4：`sniff_override_destination: true` 可能干扰DNS分流

**问题**: 此选项用嗅探到的域名覆盖目标IP。对于 direct 出站的国内流量，这意味着DNS污染的IP会被覆盖为真实域名——但 direct 出站需要用IP连接，覆盖后反而可能导致连接问题。

**修复**: 改为 `false`。DNS分流已通过 hijack-dns + dns.rules 处理，不需要 sniff_override_destination。

---

### 缺陷 #5：setBlocking 移除后仍可能有问题

**已修复** (上一版): `setBlocking(true)` 已移除。但仅此一项不够——即使不 blocking，direct 出站仍会回环（缺陷#1）。

---

## 二、完整流量环路示意

### 当前（坏的）:
```
所有App → Android VPN路由(0.0.0.0/0) → TUN fd
→ sing-box读取TUN数据包
→ 路由规则匹配:
  ├── ChatGPT域名 → proxy出站 → VLESS → 东京VPS ✅
  ├── CN域名/IP → direct出站 → 新socket → 被VPN路由捕获 → TUN → 死循环 ❌
  └── DNS查询 → outbound:dns-out → 不完整处理 → DNS解析失败 ❌
```

### 修复后:
```
所有App → Android VPN路由(0.0.0.0/0) → TUN fd
→ sing-box读取TUN数据包
→ 路由规则匹配:
  ├── DNS查询 → hijack-dns → sing-box DNS模块
  │   ├── CN域名 → 223.5.5.5(direct) → protect+auto_detect_interface → 物理网卡 ✅
  │   └── 外网域名 → 1.1.1.1(proxy) → VLESS → 东京VPS ✅
  ├── ChatGPT域名 → proxy出站 → VLESS ✅
  └── CN域名/IP → direct出站 → auto_detect_interface绑定wlan0 → 物理网卡 ✅
```

---

## 三、修复方案

### TunnelVpnService.kt 改动汇总

| # | 位置 | 当前值 | 修复值 | 原因 |
|---|------|--------|--------|------|
| 1 | route配置 | 缺少auto_detect_interface | `"auto_detect_interface": true` | **致命**: direct出站不绑定物理网卡导致回环 |
| 2 | DNS规则 | `"outbound": "dns-out"` | `"action": "hijack-dns"` | **致命**: TUN模式DNS劫持不正确 |
| 3 | stack | `"system"` | `"mixed"` | **严重**: system栈UDP在Android不可靠 |
| 4 | sniff_override_destination | `true` | `false` | DNS分流已独立处理，不需要覆盖 |
| 5 | VPN Builder DNS | `1.0.0.1` | 保持（sing-box接管DNS） | sing-box通过hijack-dns接管所有DNS |

### 修复后的完整 sing-box 配置:

```json
{
  "log": {"level": "warn"},
  "inbounds": [{
    "type": "tun",
    "tag": "tun-in",
    "inet4_address": "172.19.0.1/30",
    "auto_route": true,
    "strict_route": false,
    "stack": "mixed",
    "sniff": true,
    "sniff_override_destination": false,
    "fd": TUN_FD
  }],
  "outbounds": [
    {
      "type": "vless",
      "tag": "proxy",
      "server": "SERVER",
      "server_port": 443,
      "uuid": "UUID",
      "flow": "xtls-rprx-vision",
      "tls": {
        "enabled": true,
        "server_name": "www.cloudflare.com",
        "utls": {"enabled": true, "fingerprint": "chrome"},
        "reality": {
          "enabled": true,
          "public_key": "PUBKEY",
          "short_id": "SHORTID"
        }
      }
    },
    {"type": "direct", "tag": "direct", "domain_strategy": "prefer_ipv4"},
    {"type": "dns", "tag": "dns-out"}
  ],
  "dns": {
    "servers": [
      {"tag": "proxy-dns", "address": "https://1.1.1.1/dns-query", "detour": "proxy"},
      {"tag": "local-dns", "address": "223.5.5.5", "detour": "direct"}
    ],
    "rules": [
      {"geosite": ["cn"], "server": "local-dns", "disable_cache": true}
    ],
    "final": "proxy-dns",
    "strategy": "prefer_ipv4"
  },
  "route": {
    "auto_detect_interface": true,
    "geoip": {"path": "geoip.db路径"},
    "geosite": {"path": "geosite.db路径"},
    "rules": [
      {"protocol": "dns", "action": "hijack-dns"},
      {"domain_suffix": ["chatgpt.com",...], "outbound": "proxy"},
      {"ip_is_private": true, "outbound": "direct"},
      {"geoip": ["cn"], "outbound": "direct"},
      {"geosite": ["cn"], "outbound": "direct"}
    ],
    "final": "direct"
  }
}
```

---

## 四、VpnService.protect() 问题（潜在风险）

### 背景
Android的VPN框架会将所有流量（包括sing-box自己创建的socket）重定向到TUN。官方的 SFA（sing-box for Android）客户端会在内部自动调用 `VpnService.protect()` 来豁免出站socket。

### 当前风险
我们的app以CLI子进程方式运行sing-box，**无法直接调用protect()**。依赖 `auto_detect_interface` 绑定物理网卡来绕开回环。这在大多数设备上应该可行，但部分ROM可能有兼容问题。

### 后备方案（如果auto_detect_interface不够）
在VpnService中实现socket保护：
1. sing-box通过本地SOCKS代理出站
2. VpnService在SOCKS代理中对每个socket调用protect()
3. 需要额外开发量，暂不实施

---

## 五、验证清单

修复后需验证：
- [ ] VPN开启后微信消息正常收发
- [ ] 国内网站（baidu.com, taobao.com）正常访问
- [ ] ChatGPT.com 可正常访问
- [ ] DNS不泄漏（访问chatgpt.com走1.1.1.1而非223.5.5.5）
- [ ] VPN关闭后一切恢复正常
- [ ] 不同网络环境（WiFi/4G）均正常

---

## 六、参考资料

- sing-box TUN文档: https://sing-box.sagernet.org/configuration/inbound/tun/
- sing-box Route文档: https://sing-box.sagernet.org/configuration/route/
- SFA源码: https://github.com/SagerNet/sing-box-for-android
- Android VpnService: https://developer.android.com/reference/android/net/VpnService
- sing-box GitHub Issues: #1502, #2740, #3382, #3387, #3701
