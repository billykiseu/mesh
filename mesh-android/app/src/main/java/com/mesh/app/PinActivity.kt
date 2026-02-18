package com.mesh.app

import android.content.Intent
import android.content.SharedPreferences
import android.os.Bundle
import android.os.CountDownTimer
import android.widget.*
import androidx.appcompat.app.AppCompatActivity
import java.security.MessageDigest

class PinActivity : AppCompatActivity() {

    companion object {
        private const val PREFS = "mesh_pin_prefs"
        private const val KEY_PIN_HASH = "pin_hash"
        private const val KEY_ATTEMPTS = "pin_attempts"
        private const val KEY_LOCKOUT_UNTIL = "lockout_until"
        private const val MAX_ATTEMPTS = 5
        private const val LOCKOUT_MS = 30_000L

        fun isPinSet(prefs: SharedPreferences): Boolean {
            return prefs.getString(KEY_PIN_HASH, null) != null
        }

        fun hashPin(pin: String): String {
            val digest = MessageDigest.getInstance("SHA-256")
            val hash = digest.digest(pin.toByteArray())
            return hash.joinToString("") { "%02x".format(it) }
        }
    }

    private lateinit var prefs: SharedPreferences
    private lateinit var pinInput: EditText
    private lateinit var pinButton: Button
    private lateinit var statusText: TextView
    private lateinit var titleText: TextView
    private var isSetup = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        prefs = getSharedPreferences(PREFS, MODE_PRIVATE)
        isSetup = !isPinSet(prefs)

        // If no PIN set and this is first launch, go straight to main
        // (PIN is optional - users set it in settings)
        if (isSetup) {
            proceed()
            return
        }

        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(64, 200, 64, 64)
            gravity = android.view.Gravity.CENTER_HORIZONTAL
        }

        titleText = TextView(this).apply {
            text = if (isSetup) "Create PIN" else "Enter PIN"
            textSize = 24f
            gravity = android.view.Gravity.CENTER
            setPadding(0, 0, 0, 32)
        }
        layout.addView(titleText)

        statusText = TextView(this).apply {
            textSize = 14f
            gravity = android.view.Gravity.CENTER
            setPadding(0, 0, 0, 16)
        }
        layout.addView(statusText)

        pinInput = EditText(this).apply {
            hint = "4-6 digit PIN"
            inputType = android.text.InputType.TYPE_CLASS_NUMBER or
                    android.text.InputType.TYPE_NUMBER_VARIATION_PASSWORD
            gravity = android.view.Gravity.CENTER
            textSize = 24f
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                setMargins(0, 0, 0, 32)
            }
        }
        layout.addView(pinInput)

        pinButton = Button(this).apply {
            text = if (isSetup) "Set PIN" else "Unlock"
            setOnClickListener { handlePin() }
        }
        layout.addView(pinButton)

        if (!isSetup) {
            val skipBtn = Button(this).apply {
                text = "Remove PIN"
                setOnClickListener {
                    prefs.edit().remove(KEY_PIN_HASH).remove(KEY_ATTEMPTS).apply()
                    proceed()
                }
            }
            // Don't show remove by default - only in settings
        }

        setContentView(layout)
        checkLockout()
    }

    private fun handlePin() {
        val pin = pinInput.text.toString()

        if (pin.length < 4 || pin.length > 6) {
            statusText.text = "PIN must be 4-6 digits"
            return
        }

        if (isSetup) {
            // Store new PIN
            prefs.edit()
                .putString(KEY_PIN_HASH, hashPin(pin))
                .putInt(KEY_ATTEMPTS, 0)
                .apply()
            proceed()
        } else {
            // Verify PIN
            val stored = prefs.getString(KEY_PIN_HASH, null)
            if (hashPin(pin) == stored) {
                prefs.edit().putInt(KEY_ATTEMPTS, 0).apply()
                proceed()
            } else {
                val attempts = prefs.getInt(KEY_ATTEMPTS, 0) + 1
                prefs.edit().putInt(KEY_ATTEMPTS, attempts).apply()

                if (attempts >= MAX_ATTEMPTS) {
                    val until = System.currentTimeMillis() + LOCKOUT_MS
                    prefs.edit().putLong(KEY_LOCKOUT_UNTIL, until).apply()
                    startLockout(LOCKOUT_MS)
                } else {
                    statusText.text = "Wrong PIN (${MAX_ATTEMPTS - attempts} attempts left)"
                    pinInput.text.clear()
                }
            }
        }
    }

    private fun checkLockout() {
        val until = prefs.getLong(KEY_LOCKOUT_UNTIL, 0)
        val remaining = until - System.currentTimeMillis()
        if (remaining > 0) {
            startLockout(remaining)
        }
    }

    private fun startLockout(durationMs: Long) {
        pinInput.isEnabled = false
        pinButton.isEnabled = false
        object : CountDownTimer(durationMs, 1000) {
            override fun onTick(remaining: Long) {
                statusText.text = "Locked out. Try again in ${remaining / 1000}s"
            }
            override fun onFinish() {
                pinInput.isEnabled = true
                pinButton.isEnabled = true
                statusText.text = ""
                prefs.edit().putInt(KEY_ATTEMPTS, 0).remove(KEY_LOCKOUT_UNTIL).apply()
            }
        }.start()
    }

    private fun proceed() {
        startActivity(Intent(this, MainActivity::class.java))
        finish()
    }
}
