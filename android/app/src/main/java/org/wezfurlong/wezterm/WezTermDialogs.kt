package org.wezfurlong.wezterm

import android.app.Activity
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Color
import android.text.InputType
import android.util.TypedValue
import android.view.View
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.widget.SwitchCompat
import org.json.JSONObject

/**
 * The small native dialogs the Rust side asks for.
 *
 * Everything else in the app is drawn by Rust into the Activity's surface, which
 * keeps one coordinate system for touch, resizing and rendering. Text entry is
 * the exception: a host editor needs form fields with working IME editing,
 * selection and a cursor, and a password field needs masking and to be kept away
 * from autofill and from the IME's learned vocabulary. None of that is worth
 * reimplementing in a renderer.
 *
 * An AlertDialog is a window of its own above the surface, so unlike wrapping
 * the GameActivity surface in a DrawerLayout it introduces no surface z-order,
 * gesture dispatch or terminal-size problems.
 *
 * The spec is JSON built by the Rust side; see wezterm-gui/src/dialog.rs, which
 * owns the schema. This file only renders it and hands back the values.
 */
object WezTermDialogs {

    /** Field kinds the spec may ask for. Anything unknown is treated as text. */
    private const val KIND_TEXT = "text"
    private const val KIND_NUMBER = "number"
    private const val KIND_PASSWORD = "password"
    private const val KIND_SECRET_MULTILINE = "secret_multiline"
    private const val KIND_TOGGLE = "toggle"
    private const val KIND_CHOICE = "choice"

    /**
     * Present a dialog and report the outcome exactly once.
     *
     * [respond] is called with the values on submit, or with null on any form of
     * dismissal. Cancelling has no effect beyond failing whatever operation was
     * waiting: the Rust side treats a null as "the user said no".
     */
    fun show(activity: Activity, spec: String, respond: (Map<String, String>?) -> Unit) {
        val parsed = try {
            JSONObject(spec)
        } catch (err: Exception) {
            // A spec we cannot read is a bug on the Rust side, but reporting a
            // cancellation keeps the caller from waiting forever for a dialog
            // that was never shown. The message is safe to log: a spec carries
            // labels and existing values, never an answer the user has typed.
            android.util.Log.e("wezterm", "unreadable dialog spec: $spec", err)
            respond(null)
            return
        }

        val fields = mutableListOf<Field>()
        val fieldsJson = parsed.optJSONArray("fields")
        if (fieldsJson != null) {
            for (i in 0 until fieldsJson.length()) {
                val field = fieldsJson.optJSONObject(i) ?: continue
                val options = mutableListOf<Choice>()
                field.optJSONArray("options")?.let { array ->
                    for (j in 0 until array.length()) {
                        val option = array.optJSONObject(j) ?: continue
                        options.add(
                            Choice(option.optString("value"), option.optString("label")),
                        )
                    }
                }
                fields.add(
                    Field(
                        key = field.optString("key"),
                        label = field.optString("label"),
                        kind = field.optString("kind", KIND_TEXT),
                        value = field.optString("value"),
                        hint = field.optString("hint").ifEmpty { null },
                        error = field.optString("error").ifEmpty { null },
                        options = options,
                    ),
                )
            }
        }

        // How to read each field back, keyed the same way as the spec. Not every
        // field is an EditText any more: a toggle is a Switch and a choice is a
        // Spinner, and neither has text to read.
        val readers = mutableMapOf<String, () -> String>()
        // Only the text editors holding something secret, which are the ones
        // that have to be wiped once the answer has been handed over.
        val sensitive = mutableListOf<EditText>()
        val body = buildBody(
            activity,
            parsed.optString("message").ifEmpty { null },
            fields,
            readers,
            sensitive,
        )

        // Exactly once: a dialog can be submitted, cancelled, or dismissed by
        // the system, and more than one of those can fire for a single dialog.
        // A second answer to the same request id would be dropped by the Rust
        // side anyway, but the sensitive-field wipe below must not be skipped
        // because a later path assumed it had already run.
        var answered = false
        val answer = { values: Map<String, String>? ->
            if (!answered) {
                answered = true
                respond(values)
                // Do not leave a password or a private key sitting in a view
                // hierarchy that the dialog may keep alive.
                for (editor in sensitive) {
                    editor.setText("")
                }
            }
        }

        val grave = parsed.optBoolean("grave", false)
        val builder = AlertDialog.Builder(activity)
            .setTitle(parsed.optString("title"))
            .setView(body)
            .setPositiveButton(parsed.optString("submit_label", "OK")) { _, _ ->
                val values = fields.associate { field ->
                    field.key to (readers[field.key]?.invoke() ?: "")
                }
                if (parsed.optBoolean("clear_clipboard_on_submit", false)) {
                    clearClipboard(activity)
                }
                answer(values)
            }
            .setNegativeButton(parsed.optString("cancel_label", "Cancel")) { _, _ -> answer(null) }
            .setOnCancelListener { answer(null) }
            // A warning that can be dismissed by a stray tap outside it, or by
            // the back gesture, has not been read. Host key verification is the
            // case this exists for.
            .setCancelable(!grave)

        val dialog = builder.create()
        dialog.setCanceledOnTouchOutside(!grave)
        dialog.show()
    }

    /**
     * Encode submitted values as the JSON the Rust side expects.
     *
     * Built here rather than by string concatenation at the call site because a
     * password containing a quote or a backslash would otherwise produce a
     * document that does not parse -- and the value that broke it must never
     * appear in a log to explain why.
     */
    fun encode(values: Map<String, String>): String {
        val payload = JSONObject()
        for ((key, value) in values) {
            payload.put(key, value)
        }
        return JSONObject().put("values", payload).toString()
    }

    private data class Choice(val value: String, val label: String)

    private data class Field(
        val key: String,
        val label: String,
        val kind: String,
        val value: String,
        val hint: String?,
        val error: String?,
        val options: List<Choice>,
    )

    private fun isSensitive(kind: String) =
        kind == KIND_PASSWORD || kind == KIND_SECRET_MULTILINE

    private fun buildBody(
        context: Context,
        message: String?,
        fields: List<Field>,
        readers: MutableMap<String, () -> String>,
        sensitive: MutableList<EditText>,
    ): View {
        val pad = dp(context, 16)
        val column = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, dp(context, 8), pad, 0)
        }

        if (message != null) {
            column.addView(
                TextView(context).apply {
                    text = message
                    setPadding(0, 0, 0, dp(context, 8))
                },
            )
        }

        for (field in fields) {
            // A switch carries its own label, so a separate caption above it
            // would read as the label twice.
            if (field.kind != KIND_TOGGLE) {
                column.addView(
                    TextView(context).apply {
                        text = field.label
                        setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
                    },
                )
            }

            when (field.kind) {
                KIND_TOGGLE -> {
                    val toggle = SwitchCompat(context).apply {
                        text = field.label
                        isChecked = field.value.equals("true", ignoreCase = true)
                    }
                    readers[field.key] = { if (toggle.isChecked) "true" else "false" }
                    column.addView(toggle)
                    field.hint?.let { hint ->
                        column.addView(
                            TextView(context).apply {
                                text = hint
                                setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
                            },
                        )
                    }
                }

                KIND_CHOICE -> {
                    val options = field.options
                    val spinner = Spinner(context).apply {
                        adapter = ArrayAdapter(
                            context,
                            android.R.layout.simple_spinner_item,
                            options.map { it.label },
                        ).apply {
                            setDropDownViewResource(
                                android.R.layout.simple_spinner_dropdown_item,
                            )
                        }
                        val selected = options.indexOfFirst { it.value == field.value }
                        if (selected >= 0) {
                            setSelection(selected)
                        }
                    }
                    readers[field.key] = {
                        // The position, not the visible label: two keys may
                        // legitimately be given the same name, and it is the
                        // value behind the row that identifies which one.
                        options.getOrNull(spinner.selectedItemPosition)?.value ?: ""
                    }
                    column.addView(spinner)
                }

                else -> {
                    val editor = EditText(context).apply {
                        setText(field.value)
                        applyKind(this, field.kind)
                        field.hint?.let { hint = it }
                        // A key or a password must not be offered to autofill,
                        // and must not be learned by the IME for suggestion.
                        if (isSensitive(field.kind)) {
                            importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO
                            setImportantForAccessibility(
                                View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS,
                            )
                        }
                    }
                    if (isSensitive(field.kind)) {
                        sensitive.add(editor)
                    }
                    readers[field.key] = { editor.text?.toString() ?: "" }
                    column.addView(editor)
                }
            }

            // Validation errors sit beside the field they belong to, with the
            // value the user typed still in it: a dialog that clears itself to
            // report a typo makes the user retype everything.
            field.error?.let { error ->
                column.addView(
                    TextView(context).apply {
                        text = error
                        setTextColor(ERROR_COLOR)
                        setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
                    },
                )
            }
        }

        // Scrollable, because a pasted private key field plus four host fields
        // does not fit above the soft keyboard on a phone in portrait.
        return ScrollView(context).apply {
            addView(
                column,
                LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ),
            )
        }
    }

    private fun applyKind(editor: EditText, kind: String) {
        when (kind) {
            KIND_NUMBER -> {
                editor.inputType = InputType.TYPE_CLASS_NUMBER
            }
            KIND_PASSWORD -> {
                editor.inputType =
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            }
            KIND_SECRET_MULTILINE -> {
                // Not masked: a key is pasted rather than typed, and the user
                // needs to see that the paste landed. TYPE_TEXT_FLAG_NO_SUGGESTIONS
                // is what keeps a cloud-syncing IME from taking an interest.
                editor.inputType = InputType.TYPE_CLASS_TEXT or
                    InputType.TYPE_TEXT_FLAG_MULTI_LINE or
                    InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                editor.setLines(4)
                editor.maxLines = 8
            }
            else -> {
                editor.inputType = InputType.TYPE_CLASS_TEXT or
                    InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            }
        }
    }

    /**
     * Drop whatever is on the clipboard.
     *
     * Importing a private key pulls it through the clipboard, which on Android 13
     * and later also puts it in the system's clipboard preview and history, and
     * may expose it to a cloud-syncing IME. That is accepted -- the alternative,
     * with no file picker and no reachable HOME, is no key support at all -- but
     * it is not left lying there afterwards.
     */
    private fun clearClipboard(context: Context) {
        val clipboard =
            context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
        try {
            clipboard.clearPrimaryClip()
        } catch (err: Exception) {
            // Not fatal, and deliberately not logged with any content.
            android.util.Log.w("wezterm", "could not clear the clipboard")
        }
    }

    private fun dp(context: Context, value: Int): Int =
        (value * context.resources.displayMetrics.density).toInt()

    private val ERROR_COLOR = Color.parseColor("#CF6679")
}
