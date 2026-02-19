package com.mesh.app

import android.content.Intent
import android.os.Bundle
import android.widget.*
import androidx.appcompat.app.AppCompatActivity
import androidx.viewpager2.widget.ViewPager2
import android.view.View
import android.view.ViewGroup
import android.view.LayoutInflater
import androidx.recyclerview.widget.RecyclerView

class OnboardingActivity : AppCompatActivity() {

    companion object {
        private const val PREFS = "mesh_onboarding"
        private const val KEY_COMPLETED = "onboarding_done"

        fun isOnboardingDone(activity: AppCompatActivity): Boolean {
            return activity.getSharedPreferences(PREFS, MODE_PRIVATE)
                .getBoolean(KEY_COMPLETED, false)
        }
    }

    private data class Page(val title: String, val description: String, val emoji: String)

    private val pages = listOf(
        Page(
            "Welcome to MassKritical",
            "A peer-to-peer mesh network that works without internet.\n\n" +
                    "Connect with people nearby over WiFi or hotspot. " +
                    "No servers, no accounts, no tracking.",
            "~"
        ),
        Page(
            "Discover & Connect",
            "Your device automatically discovers nearby mesh nodes.\n\n" +
                    "Walk around to find peers. The mesh grows as more people join. " +
                    "Messages relay through other nodes to reach further.",
            "#"
        ),
        Page(
            "Communicate",
            "Send text messages, voice notes, and files.\n\n" +
                    "Direct messages go to specific peers. Broadcasts reach everyone. " +
                    "Push-to-talk for real-time voice over the mesh.",
            ">"
        ),
        Page(
            "Privacy & Safety",
            "End-to-end encryption protects all messages.\n\n" +
                    "Set a PIN to lock the app. Use the NUKE button to instantly " +
                    "destroy your identity and all data if needed.",
            "!"
        ),
    )

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Skip onboarding if already completed
        if (isOnboardingDone(this)) {
            proceed()
            return
        }

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(32, 32, 32, 32)
        }

        val viewPager = ViewPager2(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f
            )
        }
        viewPager.adapter = OnboardingAdapter(pages)
        root.addView(viewPager)

        // Dot indicators
        val dotsLayout = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = android.view.Gravity.CENTER
            setPadding(0, 16, 0, 16)
        }
        val dots = mutableListOf<TextView>()
        pages.forEachIndexed { i, _ ->
            val dot = TextView(this).apply {
                text = " o "
                textSize = 16f
            }
            dots.add(dot)
            dotsLayout.addView(dot)
        }
        root.addView(dotsLayout)

        viewPager.registerOnPageChangeCallback(object : ViewPager2.OnPageChangeCallback() {
            override fun onPageSelected(position: Int) {
                dots.forEachIndexed { i, dot ->
                    dot.text = if (i == position) " * " else " o "
                }
            }
        })

        val btnLayout = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = android.view.Gravity.CENTER
        }

        val getStartedBtn = Button(this).apply {
            text = "Get Started"
            setOnClickListener {
                getSharedPreferences(PREFS, MODE_PRIVATE)
                    .edit()
                    .putBoolean(KEY_COMPLETED, true)
                    .apply()
                proceed()
            }
        }
        btnLayout.addView(getStartedBtn)
        root.addView(btnLayout)

        setContentView(root)
    }

    private fun proceed() {
        startActivity(Intent(this, PinActivity::class.java))
        finish()
    }

    private class OnboardingAdapter(private val pages: List<Page>) :
        RecyclerView.Adapter<OnboardingAdapter.VH>() {

        class VH(val layout: LinearLayout) : RecyclerView.ViewHolder(layout)

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): VH {
            val layout = LinearLayout(parent.context).apply {
                orientation = LinearLayout.VERTICAL
                gravity = android.view.Gravity.CENTER
                setPadding(48, 100, 48, 48)
                layoutParams = ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT
                )
            }
            return VH(layout)
        }

        override fun onBindViewHolder(holder: VH, position: Int) {
            val page = pages[position]
            holder.layout.removeAllViews()

            val icon = TextView(holder.layout.context).apply {
                text = page.emoji
                textSize = 48f
                gravity = android.view.Gravity.CENTER
                setPadding(0, 0, 0, 32)
            }
            holder.layout.addView(icon)

            val title = TextView(holder.layout.context).apply {
                text = page.title
                textSize = 28f
                gravity = android.view.Gravity.CENTER
                setPadding(0, 0, 0, 24)
            }
            holder.layout.addView(title)

            val desc = TextView(holder.layout.context).apply {
                text = page.description
                textSize = 16f
                gravity = android.view.Gravity.CENTER
                setLineSpacing(4f, 1.2f)
            }
            holder.layout.addView(desc)
        }

        override fun getItemCount() = pages.size
    }
}
