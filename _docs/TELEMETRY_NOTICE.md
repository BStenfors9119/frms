<!--
  frms telemetry/diagnostics disclosure. This is the user-facing source of truth;
  the wording is mirrored by the in-app first-run notice
  (src/ui/telemetry_notice.rs) and the Profile > Privacy text.

  Presentation: a one-time modal on first launch. Both choices ("Keep it on" /
  "Turn off optional data") acknowledge it so it won't reappear; the opt-out
  also lives in Profile. Two tiers: install + app-open counts always report and
  are disclosed here; everything else is on by default and opt-out. This
  discloses rather than hard-gates — unlike the install-time NDA accept/decline
  (see _docs/NDA.md).
-->

# frms — Telemetry & Diagnostics Notice

To improve frms and fix problems, the app collects a small amount of
**anonymous** usage and diagnostic data. This notice explains what it does and
does not collect, and what you can turn off.

## What it always collects

frms reports that it was **installed**, and reports each time the app is
**opened**. This is the licence and adoption count, and it is not covered by
the opt-out below. Each of those reports carries only:

- An **anonymous install id** (a random value — not tied to you or your account)
- The app **version**, your **operating system**, and **CPU architecture**

## What you can turn off

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

The optional data above is **on by default**, and you can turn it off at any
time — the install and app-open counts continue either way:

- In the app: **Profile tab → Privacy → uncheck "Share anonymous usage data."**
- Before launch: set the environment variable **`DO_NOT_TRACK=1`** (or
  **`FRMS_NO_TELEMETRY=1`**).

By continuing with the installation, you acknowledge this notice. You can change
your choice at any time as described above.

**Contact:** Soda Pop Systems — admin@sodapopsystems.com
