package com.xiaziteam.chatgptshell

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.net.ConnectivityManager
import android.net.VpnService
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import io.nekohasekai.libbox.CommandServer
import io.nekohasekai.libbox.CommandServerHandler
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.OverrideOptions
import io.nekohasekai.libbox.PlatformInterface
import io.nekohasekai.libbox.SetupOptions
import io.nekohasekai.libbox.SystemProxyStatus
import io.nekohasekai.libbox.TunOptions
import io.nekohasekai.libbox.InterfaceUpdateListener
import io.nekohasekai.libbox.NetworkInterfaceIterator
import io.nekohasekai.libbox.WIFIState
import io.nekohasekai.libbox.LocalDNSTransport
import io.nekohasekai.libbox.NeighborUpdateListener
import io.nekohasekai.libbox.PlatformUser
import io.nekohasekai.libbox.BridgeOptions
import io.nekohasekai.libbox.BridgeSession
import io.nekohasekai.libbox.ConnectionOwner
import io.nekohasekai.libbox.ShellSession
import io.nekohasekai.libbox.StringIterator
import android.system.OsConstants
import java.io.File

class TunnelVpnService :
    VpnService(),
    PlatformInterface {

    companion object {
        private const val TAG = "TunnelVpn"
        private const val NOTIF_CHANNEL_ID = "tunnel_vpn"
        private const val NOTIF_ID = 1
    }

    private var fileDescriptor: ParcelFileDescriptor? = null
    private var commandServer: CommandServer? = null
    private var tunnelPlan: String = "basic"
    private var libboxSetup = false
    private var protectMark: Int = 0

    private val handler = object : CommandServerHandler {
        override fun serviceStop() {
            closeAndStop()
        }

        override fun serviceReload() {}

        override fun getSystemProxyStatus(): SystemProxyStatus? = null

        override fun setSystemProxyEnabled(isEnabled: Boolean) {}

        override fun connectSSHAgent(): Int = -1

        override fun triggerNativeCrash() {}

        override fun writeDebugMessage(message: String?) {}
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null) return START_NOT_STICKY

        val server = intent.getStringExtra("server") ?: return START_NOT_STICKY
        val serverPort = intent.getIntExtra("server_port", 443)
        val uuid = intent.getStringExtra("uuid") ?: return START_NOT_STICKY
        val flow = intent.getStringExtra("flow") ?: "xtls-rprx-vision"
        val serverName = intent.getStringExtra("server_name") ?: "www.cloudflare.com"
        val publicKey = intent.getStringExtra("public_key") ?: return START_NOT_STICKY
        val shortId = intent.getStringExtra("short_id") ?: ""
        val routeDomains = intent.getStringArrayListExtra("route_domains") ?: emptyList()
        tunnelPlan = intent.getStringExtra("plan") ?: "basic"

        createNotificationChannel()
        val notif = buildNotification("正在启动...")
        startForeground(NOTIF_ID, notif)

        Thread {
            try {
                startTunnel(server, serverPort, uuid, flow, serverName, publicKey, shortId, routeDomains)
            } catch (e: Exception) {
                Log.e(TAG, "Tunnel error", e)
                stopSelf()
            }
        }.start()

        return START_STICKY
    }

    private fun startTunnel(
        server: String,
        serverPort: Int,
        uuid: String,
        flow: String,
        serverName: String,
        publicKey: String,
        shortId: String,
        routeDomains: List<String>
    ) {
        val configContent = generateConfig(server, serverPort, uuid, flow, serverName, publicKey, shortId, routeDomains, getProtectMark())

        val configFile = File(cacheDir, "singbox-config.json")
        configFile.writeText(configContent)

        if (!libboxSetup) {
            val baseDir = filesDir
            baseDir.mkdirs()
            val workingDir = getExternalFilesDir(null) ?: filesDir
            workingDir.mkdirs()
            val tempDir = cacheDir
            tempDir.mkdirs()
            val setupOptions = SetupOptions().also {
                it.basePath = baseDir.absolutePath
                it.workingPath = workingDir.absolutePath
                it.tempPath = tempDir.absolutePath
            }
            Libbox.setup(setupOptions)
            libboxSetup = true
            Log.i(TAG, "libbox setup: base=${baseDir.absolutePath} working=${workingDir.absolutePath} temp=${tempDir.absolutePath}")
        }

        Libbox.promoteOOMDraft()

        val cmdServer = CommandServer(handler, this)
        cmdServer.start()
        commandServer = cmdServer

        try {
            cmdServer.startOrReloadService(configContent, OverrideOptions())
            Log.i(TAG, "libbox service started with gVisor stack")
            updateNotification("VPN隧道运行中")
            openTargetSite()
        } catch (e: Exception) {
            Log.e(TAG, "libbox startOrReloadService failed", e)
            closeAndStop()
        }
    }

    private fun generateConfig(
        server: String,
        serverPort: Int,
        uuid: String,
        flow: String,
        serverName: String,
        publicKey: String,
        shortId: String,
        routeDomains: List<String>,
        mark: Int
    ): String {
        val domainsArray = routeDomains.map { "\"$it\"" }.joinToString(",")

        return """
{
  "log": {"level": "debug", "output": "/data/user/0/com.xiaziteam.chatgptshell/files/box.log", "timestamp": true},
   "inbounds": [{
    "type": "tun",
    "tag": "tun-in",
     "address": ["172.19.0.1/30"],
     "auto_route": true,
     "strict_route": false,
     "stack": "gvisor"
   }],
  "outbounds": [
    {
      "type": "vless",
      "tag": "proxy",
      "server": "$server",
      "server_port": $serverPort,
      "uuid": "$uuid",
      "flow": "$flow",
      "tls": {
        "enabled": true,
        "server_name": "$serverName",
        "utls": {"enabled": true, "fingerprint": "chrome"},
        "reality": {
          "enabled": true,
          "public_key": "$publicKey",
          "short_id": "$shortId"
        }
      }
    },
    {"type": "direct", "tag": "direct"}
  ],
   "dns": {
     "servers": [
       {"type": "udp", "tag": "local-dns", "server": "223.5.5.5"},
       {"type": "https", "tag": "proxy-dns", "server": "1.1.1.1", "domain_resolver": "local-dns", "detour": "proxy"}
     ],
     "rules": [
       {"outbound": "any", "server": "local-dns"},
       {"domain_suffix": [$domainsArray], "server": "proxy-dns"}
     ],
     "final": "local-dns",
     "strategy": "prefer_ipv4"
   },
   "route": {
    "auto_detect_interface": true,
    "rules": [
      {"action": "sniff"},
      {"protocol": "dns", "action": "hijack-dns"},
      {"domain_suffix": [$domainsArray], "outbound": "proxy"},
      {"ip_is_private": true, "outbound": "direct"}
    ],
    "final": "direct"
  }
}
""".trimIndent()
    }

    private fun getProtectMark(): Int {
        val cm = getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
        for (network in cm.allNetworks) {
            val caps = cm.getNetworkCapabilities(network)
            if (caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) {
                val netId = network.toString().removePrefix("Network{").removeSuffix("}").toIntOrNull() ?: 0
                protectMark = 0x100000 or netId
                Log.i(TAG, "protectMark: 0x${Integer.toHexString(protectMark)} (netId=$netId)")
                return protectMark
            }
        }
        val active = cm.activeNetwork
        if (active != null) {
            val netId = active.toString().removePrefix("Network{").removeSuffix("}").toIntOrNull() ?: 0
            protectMark = 0x100000 or netId
            Log.i(TAG, "protectMark: 0x${Integer.toHexString(protectMark)} (active netId=$netId)")
        }
        return protectMark
    }

    private fun bindProcessToUnderlyingNetwork() {
        val cm = getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
        for (network in cm.allNetworks) {
            val caps = cm.getNetworkCapabilities(network)
            if (caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) {
                val result = cm.bindProcessToNetwork(network)
                Log.i(TAG, "bindProcessToNetwork(wifi=$network) = $result")
                return
            }
        }
        Log.w(TAG, "no wifi network found, trying activeNetwork")
        val active = cm.activeNetwork
        if (active != null) {
            cm.bindProcessToNetwork(active)
            Log.i(TAG, "bindProcessToNetwork(active=$active)")
        }
    }

    // --- PlatformInterface implementation ---

    override fun usePlatformAutoDetectInterfaceControl(): Boolean {
        Log.e(TAG, "usePlatformAutoDetectInterfaceControl called -> true")
        return true
    }

    override fun autoDetectInterfaceControl(fd: Int) {
        protect(fd)
        Log.e(TAG, "autoDetectInterfaceControl: protect(fd=$fd)")
    }

    override fun openTun(options: TunOptions): Int {
        if (prepare(this) != null) error("android: missing vpn permission")

        val builder = Builder()
            .setSession("ChatGPT虾壳")
            .setMtu(options.mtu)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            builder.setMetered(false)
        }

        val inet4Address = options.inet4Address
        while (inet4Address.hasNext()) {
            val address = inet4Address.next()
            Log.d(TAG, "addAddress: ${address.address()}/${address.prefix()}")
            builder.addAddress(address.address(), address.prefix())
        }

        val inet6Address = options.inet6Address
        while (inet6Address.hasNext()) {
            val address = inet6Address.next()
            Log.d(TAG, "addAddress6: ${address.address()}/${address.prefix()}")
            builder.addAddress(address.address(), address.prefix())
        }

        if (options.autoRoute) {
            val dnsServerAddress = options.dnsServerAddress
            while (dnsServerAddress.hasNext()) {
                val dns = dnsServerAddress.next()
                Log.d(TAG, "addDnsServer: $dns")
                builder.addDnsServer(dns)
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                val inet4RouteAddress = options.inet4RouteAddress
                if (inet4RouteAddress.hasNext()) {
                    while (inet4RouteAddress.hasNext()) {
                        val prefix = inet4RouteAddress.next()
                        Log.d(TAG, "addRoute: ${prefix.address()}/${prefix.prefix()}")
                        builder.addRoute(prefix.address(), prefix.prefix())
                    }
                } else if (options.inet4Address.hasNext()) {
                    Log.d(TAG, "addRoute: 0.0.0.0/0 (default)")
                    builder.addRoute("0.0.0.0", 0)
                }
                val inet4RouteExcludeAddress = options.inet4RouteExcludeAddress
                while (inet4RouteExcludeAddress.hasNext()) {
                    val prefix = inet4RouteExcludeAddress.next()
                    Log.d(TAG, "EXCLUDE route: ${prefix.address()}/${prefix.prefix()}")
                }
            } else {
                val inet4RouteRange = options.inet4RouteRange
                if (inet4RouteRange.hasNext()) {
                    while (inet4RouteRange.hasNext()) {
                        val address = inet4RouteRange.next()
                        Log.d(TAG, "addRouteRange: ${address.address()}/${address.prefix()}")
                        builder.addRoute(address.address(), address.prefix())
                    }
                } else if (options.inet4Address.hasNext()) {
                    Log.d(TAG, "addRouteRange: 0.0.0.0/0 (default)")
                    builder.addRoute("0.0.0.0", 0)
                }
            }
        }

        builder.addDisallowedApplication(packageName)
        Log.i(TAG, "addDisallowedApplication: $packageName")

        val pfd = builder.establish() ?: error("android: vpn establish failed")
        fileDescriptor = pfd
        Log.i(TAG, "TUN established fd=${pfd.fd}")
        return pfd.fd
    }

    override fun useProcFS(): Boolean = Build.VERSION.SDK_INT < Build.VERSION_CODES.Q

    override fun underNetworkExtension(): Boolean = false

    override fun includeAllNetworks(): Boolean = false

    override fun clearDNSCache() {}

    override fun sendNotification(notification: io.nekohasekai.libbox.Notification) {
        updateNotification(notification.body)
    }

    override fun cancelNotification(identifier: String, typeID: Int) {}

    override fun findConnectionOwner(
        ipProtocol: Int,
        sourceAddress: String,
        sourcePort: Int,
        destinationAddress: String,
        destinationPort: Int
    ): ConnectionOwner {
        error("not supported")
    }

    override fun checkPlatformShell() {}

    private var defaultInterfaceMonitor: Any? = null
    private var interfaceListener: InterfaceUpdateListener? = null

    private fun notifyInterfaceUpdate() {
        val listener = interfaceListener ?: return
        try {
            val cm = getSystemService(android.net.ConnectivityManager::class.java)
            val network = cm.activeNetwork
            val linkProps = network?.let { cm.getLinkProperties(it) }
            val ifName = linkProps?.interfaceName ?: "wlan0"
            val ifIndex = try {
                java.net.NetworkInterface.getByName(ifName)?.index ?: 0
            } catch (_: Exception) { 0 }
            val isExpensive = network?.let { cm.getNetworkCapabilities(it) }?.let { caps ->
                !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
            } ?: false
            listener.updateDefaultInterface(ifName, ifIndex, isExpensive, false)
            Log.e(TAG, "updateDefaultInterface: name=$ifName index=$ifIndex expensive=$isExpensive")
        } catch (e: Exception) {
            Log.e(TAG, "updateDefaultInterface error", e)
        }
    }

    override fun startDefaultInterfaceMonitor(listener: InterfaceUpdateListener?) {
        if (listener == null) return
        interfaceListener = listener
        val cm = getSystemService(android.net.ConnectivityManager::class.java)
        val callback = object : android.net.ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                notifyInterfaceUpdate()
            }
            override fun onLost(network: Network) {
                notifyInterfaceUpdate()
            }
        }
        cm.registerNetworkCallback(
            NetworkRequest.Builder()
                .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                .build(),
            callback
        )
        defaultInterfaceMonitor = callback
        notifyInterfaceUpdate()
        Log.e(TAG, "startDefaultInterfaceMonitor: registered")
    }

    override fun closeDefaultInterfaceMonitor(listener: InterfaceUpdateListener?) {
        val callback = defaultInterfaceMonitor ?: return
        val cm = getSystemService(android.net.ConnectivityManager::class.java)
        cm.unregisterNetworkCallback(callback as android.net.ConnectivityManager.NetworkCallback)
        defaultInterfaceMonitor = null
        interfaceListener = null
        Log.e(TAG, "closeDefaultInterfaceMonitor: unregistered")
    }

    override fun getInterfaces(): NetworkInterfaceIterator? {
        Log.e(TAG, "getInterfaces called")
        return try {
            val cm = getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
            val networks = cm.allNetworks
            val jniInterfaces = java.util.Collections.list(java.net.NetworkInterface.getNetworkInterfaces())
            val interfaces = mutableListOf<io.nekohasekai.libbox.NetworkInterface>()
            for (network in networks) {
                val linkProps = cm.getLinkProperties(network) ?: continue
                val caps = cm.getNetworkCapabilities(network) ?: continue
                val ifName = linkProps.interfaceName ?: continue
                val jniIf = jniInterfaces.find { it.name == ifName } ?: continue
                val boxIf = io.nekohasekai.libbox.NetworkInterface().apply {
                    name = ifName
                    index = jniIf.index
                    mtu = try { jniIf.mtu } catch (_: Exception) { 0 }
                    type = when {
                        caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> io.nekohasekai.libbox.Libbox.InterfaceTypeWIFI
                        caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> io.nekohasekai.libbox.Libbox.InterfaceTypeCellular
                        caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> io.nekohasekai.libbox.Libbox.InterfaceTypeEthernet
                        else -> io.nekohasekai.libbox.Libbox.InterfaceTypeOther
                    }
                    var dumpFlags = 0
                    if (caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)) {
                        dumpFlags = OsConstants.IFF_UP or OsConstants.IFF_RUNNING
                    }
                    if (jniIf.isLoopback) {
                        dumpFlags = dumpFlags or OsConstants.IFF_LOOPBACK
                    }
                    if (jniIf.isPointToPoint) {
                        dumpFlags = dumpFlags or OsConstants.IFF_POINTOPOINT
                    }
                    if (jniIf.supportsMulticast()) {
                        dumpFlags = dumpFlags or OsConstants.IFF_MULTICAST
                    }
                    flags = dumpFlags
                    metered = !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                    val addrList = jniIf.interfaceAddresses.map { addr ->
                        val ip = addr.address.hostAddress
                        val cleanIp = if (ip != null && ip.contains("%")) ip.substring(0, ip.indexOf("%")) else ip
                        if (cleanIp != null) "$cleanIp/${addr.networkPrefixLength}" else null
                    }.filterNotNull()
                    addresses = object : StringIterator {
                        private val iter = addrList.iterator()
                        override fun hasNext(): Boolean = iter.hasNext()
                        override fun next(): String = iter.next()
                        override fun len(): Int = addrList.size
                    }
                    val dnsList = linkProps.dnsServers?.mapNotNull { it.hostAddress } ?: emptyList()
                    dnsServer = object : StringIterator {
                        private val iter = dnsList.iterator()
                        override fun hasNext(): Boolean = iter.hasNext()
                        override fun next(): String = iter.next()
                        override fun len(): Int = dnsList.size
                    }
                    val gwList = linkProps.routes
                        ?.mapNotNull { it.gateway?.hostAddress }
                        ?: emptyList()
                    gateway = object : StringIterator {
                        private val iter = gwList.iterator()
                        override fun hasNext(): Boolean = iter.hasNext()
                        override fun next(): String = iter.next()
                        override fun len(): Int = gwList.size
                    }
                }
                interfaces.add(boxIf)
            }
            object : NetworkInterfaceIterator {
                private val iter = interfaces.iterator()
                override fun hasNext(): Boolean = iter.hasNext()
                override fun next(): io.nekohasekai.libbox.NetworkInterface = iter.next()
            }
        } catch (e: Exception) {
            Log.e(TAG, "getInterfaces error", e)
            null
        }
    }

    override fun readWIFIState(): WIFIState? = null

    override fun localDNSTransport(): LocalDNSTransport? = null

    override fun startNeighborMonitor(listener: NeighborUpdateListener?) {}

    override fun closeNeighborMonitor(listener: NeighborUpdateListener?) {}

    override fun usePlatformShell(): Boolean = false

    override fun openShellSession(
        user: PlatformUser?,
        command: String?,
        environ: StringIterator?,
        term: String?,
        rows: Int,
        cols: Int
    ): ShellSession? = null

    override fun readSystemSSHHostKey(): String = ""

    override fun lookupSFTPServer(): String = ""

    override fun tailscaleHostname(): String = "${Build.MANUFACTURER} ${Build.MODEL}"

    override fun usePlatformBridge(): Boolean = false

    override fun createBridge(options: BridgeOptions?): BridgeSession? = null

    override fun lookupUser(username: String?): PlatformUser? = null

    override fun registerMyInterface(name: String?) {}

    // --- Helpers ---

    private fun closeAndStop() {
        try {
            val pfd = fileDescriptor
            if (pfd != null) {
                pfd.close()
                fileDescriptor = null
            }
            commandServer?.close()
            commandServer = null
        } catch (e: Exception) {
            Log.w(TAG, "close error", e)
        }
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun openTargetSite() {
        val url = if (tunnelPlan == "pro") "https://google.com" else "https://chatgpt.com"
        val intent = Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url)).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        if (intent.resolveActivity(packageManager) != null) {
            startActivity(intent)
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                NOTIF_CHANNEL_ID,
                "VPN隧道",
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "VPN隧道运行通知"
                setShowBadge(false)
            }
            val nm = getSystemService(NotificationManager::class.java)
            nm.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(text: String): android.app.Notification {
        val pendingIntent = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE
        )
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            android.app.Notification.Builder(this, NOTIF_CHANNEL_ID)
                .setContentTitle("ChatGPT虾壳")
                .setContentText(text)
                .setSmallIcon(android.R.drawable.ic_lock_lock)
                .setContentIntent(pendingIntent)
                .setOngoing(true)
                .build()
        } else {
            @Suppress("DEPRECATION")
            android.app.Notification.Builder(this)
                .setContentTitle("ChatGPT虾壳")
                .setContentText(text)
                .setSmallIcon(android.R.drawable.ic_lock_lock)
                .setContentIntent(pendingIntent)
                .setOngoing(true)
                .build()
        }
    }

    private fun updateNotification(text: String) {
        val nm = getSystemService(NotificationManager::class.java)
        nm.notify(NOTIF_ID, buildNotification(text))
    }

    override fun onDestroy() {
        closeAndStop()
        super.onDestroy()
    }

    override fun onRevoke() {
        closeAndStop()
        super.onRevoke()
    }
}
