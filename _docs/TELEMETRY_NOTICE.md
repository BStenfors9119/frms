<!--
  frms telemetry/diagnostics disclosure. This is the user-facing source of truth;
  the wording is mirrored by the in-app first-run notice
  (src/ui/telemetry_notice.rs) and the Profile > Privacy text.

  Presentation: a one-time modal on first launch, shown BEFORE any telemetry is
  sent (consent before collection). Both choices ("Keep it on" / "Turn it off")
  acknowledge it so it won't reappear; the opt-out also lives in Profile.
  Telemetry is on by default, so this discloses rather than hard-gates — unlike
  the install-time NDA accept/decline (see _docs/NDA.md).
-->

# frms — Telemetry & Diagnostics Notice

To improve frms and fix problems, the app collects a small amount of
**anonymous** usage and diagnostic data. This notice explains what it does and
does not collect, and how to turn it off.

## What it collects

- An **anonymous install id** (a random value — not tied to you or your account)
- The app **version**, your **operating system**, and **CPU architecture**
- **Coarse feature usage** — e.g. which kinds of panes you open and which models
  are used, and when sessions start
- **Error and crash diagnostics** — error categories and scrubbed messages, to
  help us find and fix bugs

## What it does NOT collect

It never sends your name, account, hostname, file names, file or project paths,
the contents of your files, your prompts or chats, database data, API keys, or
any other personal information. Free-form error text is automatically scrubbed
(home folder and secret-looking tokens removed) and truncated before it is sent.

## Your choice

Telemetry is **on by default**, and you can turn it off at any time:

- In the app: **Profile tab → Privacy → uncheck "Share anonymous usage data."**
- Before launch: set the environment variable **`DO_NOT_TRACK=1`** (or
  **`FRMS_NO_TELEMETRY=1`**) and frms sends nothing.

By continuing with the installation, you acknowledge this notice. You can change
your choice at any time as described above.

**Contact:** Soda Pop Systems — admin@sodapopsystems.com
