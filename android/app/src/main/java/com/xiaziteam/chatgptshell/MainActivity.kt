package com.xiaziteam.chatgptshell

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.VpnService
import android.os.Build
import android.os.Bundle
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import com.google.gson.Gson
import kotlinx.coroutines.*
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

class MainActivity : AppCompatActivity() {

    private lateinit var codeInput: EditText
    private lateinit var activateBtn: Button
    private lateinit var statusText: TextView
    private lateinit var disconnectBtn: Button

    private val httpClient = OkHttpClient()
    private val gson = Gson()
    private val scope = MainScope()

    private var tunnelConfig: TunnelConfig? = null
    private var currentPlan: String? = null

    companion object {
        private const val VPN_REQUEST_CODE = 1001
        private const val NOTIF_PERMISSION_CODE = 1002
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        codeInput = findViewById(R.id.codeInput)
        activateBtn = findViewById(R.id.activateBtn)
        statusText = findViewById(R.id.statusText)
        disconnectBtn = findViewById(R.id.disconnectBtn)

        activateBtn.setOnClickListener { onActivate() }
        disconnectBtn.setOnClickListener { onDisconnect() }

        requestNotificationPermission()
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED
            ) {
                ActivityCompat.requestPermissions(
                    this,
                    arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                    NOTIF_PERMISSION_CODE
                )
            }
        }
    }

    private fun onActivate() {
        val code = codeInput.text.toString().trim()
        if (code.isEmpty()) {
            Toast.makeText(this, "请输入激活码", Toast.LENGTH_SHORT).show()
            return
        }

        activateBtn.isEnabled = false
        statusText.text = "状态：正在验证激活码..."
        statusText.setTextColor(ContextCompat.getColor(this, android.R.color.holo_orange_light))

        scope.launch {
            try {
                val result = validateCode(code)
                if (result.ok && result.data != null) {
                    currentPlan = result.data.plan
                    tunnelConfig = result.data.tunnel
                    statusText.text = "状态：验证成功，正在启动隧道..."
                    statusText.setTextColor(ContextCompat.getColor(this@MainActivity, android.R.color.holo_orange_light))
                    requestVpnAndStart()
                } else {
                    statusText.text = "状态：激活码无效"
                    statusText.setTextColor(ContextCompat.getColor(this@MainActivity, android.R.color.holo_red_light))
                    activateBtn.isEnabled = true
                }
            } catch (e: Exception) {
                statusText.text = "状态：验证失败 - ${e.message}"
                statusText.setTextColor(ContextCompat.getColor(this@MainActivity, android.R.color.holo_red_light))
                activateBtn.isEnabled = true
            }
        }
    }

    private suspend fun validateCode(code: String): ValidateResponse {
        return withContext(Dispatchers.IO) {
            val json = gson.toJson(mapOf("code" to code))
            val body = json.toRequestBody("application/json".toMediaType())
            val request = Request.Builder()
                .url("https://locatenotify.online/v1/code/validate")
                .post(body)
                .build()
            val response = httpClient.newCall(request).execute()
            val respBody = response.body?.string() ?: throw Exception("Empty response")
            if (!response.isSuccessful) throw Exception("HTTP ${response.code}")
            gson.fromJson(respBody, ValidateResponse::class.java)
                ?: throw Exception("Parse error")
        }
    }

    private fun requestVpnAndStart() {
        val intent = VpnService.prepare(this)
        if (intent != null) {
            startActivityForResult(intent, VPN_REQUEST_CODE)
        } else {
            startTunnel()
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == VPN_REQUEST_CODE) {
            if (resultCode == RESULT_OK) {
                startTunnel()
            } else {
                statusText.text = "状态：VPN权限被拒绝"
                statusText.setTextColor(ContextCompat.getColor(this, android.R.color.holo_red_light))
                activateBtn.isEnabled = true
            }
        }
    }

    private fun startTunnel() {
        val cfg = tunnelConfig ?: return
        val intent = Intent(this, TunnelVpnService::class.java).apply {
            putExtra("server", cfg.server)
            putExtra("server_port", cfg.server_port)
            putExtra("uuid", cfg.uuid)
            putExtra("flow", cfg.flow)
            putExtra("server_name", cfg.server_name)
            putExtra("public_key", cfg.public_key)
            putExtra("short_id", cfg.short_id)
            putStringArrayListExtra("route_domains", ArrayList(cfg.route_domains))
            putExtra("plan", currentPlan ?: "basic")
        }
        startService(intent)

        statusText.text = "状态：已连接"
        statusText.setTextColor(ContextCompat.getColor(this, android.R.color.holo_green_light))
        activateBtn.isEnabled = true
        activateBtn.visibility = View.GONE
        disconnectBtn.visibility = View.VISIBLE
    }

    private fun onDisconnect() {
        stopService(Intent(this, TunnelVpnService::class.java))
        statusText.text = "状态：已断开"
        statusText.setTextColor(ContextCompat.getColor(this, android.R.color.holo_orange_light))
        disconnectBtn.visibility = View.GONE
        activateBtn.visibility = View.VISIBLE
        activateBtn.isEnabled = true
    }

    override fun onDestroy() {
        super.onDestroy()
        scope.cancel()
    }

    data class TunnelConfig(
        val server: String,
        val server_port: Int,
        val uuid: String,
        val flow: String,
        val server_name: String,
        val public_key: String,
        val short_id: String,
        val route_domains: List<String>
    )

    data class ValidateData(
        val plan: String,
        val tunnel: TunnelConfig
    )

    data class ValidateResponse(
        val ok: Boolean,
        val data: ValidateData?
    )
}
