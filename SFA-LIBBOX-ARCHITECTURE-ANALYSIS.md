# SFA libbox + gVisor 架构分析报告 (2026-09-09)

## 核心发现
1. libbox通过gomobile bind编译成AAR,包名io.nekohasekai.libbox
2. Android侧实现PlatformInterface接口:openTun()+autoDetectInterfaceControl(fd)=protect(fd)
3. gVisor用户态协议栈处理TUN流量,TCP/UDP/DNS一次解决
4. protect()在进程内直接生效,无需跨进程fd传递

## 调用链
VpnService.establish() → fd → libbox.openTun() → gVisor netstack → Forwarder → outbound dial → protect(fd) ✅

## 虾壳迁移步骤
1. 引入libbox.aar
2. 实现PlatformInterface(openTun+protect)
3. CLI子进程→CommandServer进程内启动
4. stack:"gvisor"配置

## 关键文件
- sing-box: experimental/libbox/ (Go桥接层)
- sing-tun: stack_gvisor.go (gVisor netstack)
- SFA: bg/VPNService.kt, bg/BoxService.kt, bg/PlatformInterfaceWrapper.kt

## logcat证据(2026-09-09)
- SELinux Enforcing阻止CLI子进程执行sing-box(error=13 Permission denied)
- avc: denied { execute_no_trans }
- 结论: CLI子进程路线彻底死,libbox进程内是唯一出路
