# "frms" — Building an Interactive AI Agent Workspace (IAW)

Full disclosure, I did use AI to write ~90% of this using "frms". I edited it and added my own flavor to it, specifically describing my usage.

A few months ago I shared [the first article](https://medium.com/@sodapopsystems/frms-an-ai-workspace-by-ai-d377753c7ec4) about "frms" (pronounced "frames"), an AI Workspace I was building to fit how I actually think and work with AI. At the time it was an alpha — functional, promising, but still rough around the edges.

Since then I have been using frms daily. Not just developing it, but developing *with* it. Other projects, experiments, real work. Along the way the tool kept evolving based on what I actually needed in practice versus what I *thought* I would need sitting at a whiteboard.

Here is where things stand and what has changed.

---

## Using It for Real Changes Everything

The biggest thing I have learned is that building a tool and *using* a tool are two very different feedback loops. When I started creating "frms" it was mainly so I could 'learn' AI then quickly realized I think about projects differently than a traditional IDE. So I kept building "frms" out to match my train of thought. I call this an "IAW" or "Interactive AI Agent Workspace".

I quickly gravitated to how easy it was to work on multiple projects and multiple AI sessions. It allowed me to build out my projects but also keep building out "frms" as I ran across an item I came across as a "nice to have" more than once or twice.

A good example of this was categorizing notes. What might have been an obvious idea, I didn't cross it until the list was getting long and cumbersome. Now with the categories it has become a much more pleasant experience to take notes, shoot them over to the AI session and continue the work flow.

Another quick example is the highlighting of the current project and AI session I am in. The more projects and AI sessions I get going it was getting tough to determine which one I was working in.

---

## Not Just for Code

One thing I want to make clear — frms is not just a software development tool. I started building it as a developer, so that is where the examples naturally come from. But the workspace itself is project-based, not code-based. A "project" in frms is just a directory with things you are working on.

I am writing this article inside frms right now using the Notes tool. I have a Research agent open next to it that I have been asking to help with phrasing and structure. That has nothing to do with code — it is just writing.

Need to research a topic deeply? Open a project, spin up a Research agent, take notes as you go, and keep everything organized in one place. Planning a business proposal? Same flow — notes, AI chat for brainstorming, terminals if you need to pull data or run scripts. Studying for a certification? Create a project per subject area, use Chat mode to quiz yourself, capture key points in notes.

The AI agent modes — Build, Research, and Chat — map to different kinds of *work*, not just different kinds of *coding*. If you do anything where you are juggling multiple threads of thought, referencing files, taking notes, and talking to AI along the way, frms fits that workflow whether there is a single line of code involved or not.

---

## What Has Changed Since the First Article

### Multi-Tool AI Support

The original version was built around Claude Code exclusively. That worked, but I realized the workspace should not be locked to a single AI tool. Now frms supports pluggable AI CLIs — [Claude Code](https://claude.ai/code) and [Windsurf](https://windsurf.com/) (Devin) are both available, and adding another tool is straightforward. Each tool defines its own binary, model list, and capabilities. You pick what works for the task at hand.

### Three Agent Modes

Instead of just throwing a terminal at you with Claude running in it, frms now has three distinct agent modes:

- **Build** — A full Claude Code PTY session for hands-on development. This is the power mode for writing and editing code.
- **Research** — A headless session using a higher-capability model (Opus) for deep investigation, architecture questions, and complex analysis.
- **Chat** — A lighter headless session (Sonnet) for quick questions, brainstorming, and conversation.

Each mode is tuned for its purpose. The chat panes show tool-use activity in real-time so you can see exactly what Claude is doing — reading files, running commands, thinking — instead of staring at a blank screen wondering if anything is happening.

### Native Chat Panes

This was a big one. The original agent panes were just terminals running the Claude CLI. Now the Research and Chat modes are native UI panes that communicate with Claude through streamed JSON. Multi-turn conversations persist via session IDs, so you can pick up where you left off. There are three access tiers — read-only, read and edit, and full access including shell — so you control exactly how much autonomy each session gets.

### The Toolbox

The sidebar evolved into a proper dockable toolbox with multiple tools:

- **Profile** — Theme, font scaling, accent colors, privacy settings
- **Notes** — Project-scoped or global notes with categories, search, and a full markdown toolbar. You can even create a note directly from a terminal selection with Ctrl+Shift+N. I am even writing this article in the notes tool as I type!
- **Terminals** — Spawn additional terminal sessions per project
- **SSH** — Connect to remote machines for deployment or remote work
- **Docs** — Documentation access within the workspace that is primarily focused on _specs and _docs directories within a project

The toolbox supports split-view (two tools stacked vertically), is resizable, can be full screen width and tabs can be hidden and re-added. It docks to either side of the workspace.

### Theming That Actually Feels Right

The original version had a fixed dark theme. Now there is a full HSL color picker for accent and active-tab colors, four color palettes (Blues, Greens, Yellows, Grays) each with light and dark modes, and animated palette transitions. Global font scaling from 0.85x to 1.30x, plus separate terminal font scaling up to 3.25x for those of us who do not want to squint.

The palettes have a Scandinavian-inspired muted tone — not the typical loud neon developer theme. It is subtle but it makes a difference when you are staring at the screen all day.

### A Real Terminal

The terminal went from basic to fully capable. It is a complete VT100 implementation with 5,000 lines of scrollback, support for 16, 256, and true color, bracketed paste, mouse-drag selection, dynamic resize, and find-in-terminal with up to 2,000 highlighted matches. It handles everything I throw at it — npm builds, cargo compiles, interactive CLI tools, all of it.

### Notes System

This one came from pure necessity. I kept having thoughts, snippets, and context I needed to track per project. Notes support categories, multi-term search, collapsible groups, and multiple view modes (edit, split, preview). The markdown toolbar covers the basics — bold, italic, headings, lists, code blocks, links.

And the killer feature: select text in a terminal or AI session then hit Ctrl+Shift+N to capture it as a note instantly. Plus it works in reverse too, select note text and send it directly to the AI prompt! Create a note/edit/tweak to get that perfect prompt then send it on over.

### Windows Support

The original frms was Linux-only. Now there is a full platform abstraction layer that covers directories, shell spawning, clipboard, process management, filesystem operations, file transfer, browser discovery, and window configuration. Each seam has a trait with Unix and Windows implementations. The goal is frms running natively on Windows — specifically targeting IoT Enterprise and LTSC editions, not just desktop.

### Packaging and Distribution

What started as "just cargo build" turned into a proper distribution pipeline. There are RPM packages for Fedora, DEB packages for Debian-based systems with a custom signing keyring, and Windows executables. A self-hosted DNF repository, GitHub Releases, and installation scripts that handle dependency checks. It went from "clone and compile" to "add the repo and install" which makes a huge difference for anyone trying it out.

### Keyboard Shortcuts That Keep You in the Flow

This one is easy to overlook but it makes a huge difference in daily use. frms is built around keyboard shortcuts so you rarely have to reach for the mouse.

The one I use the most: **Ctrl+Shift+Down Arrow** to jump straight to your agent sessions. When you have multiple AI panes running — maybe a Build session compiling something and a Research session answering a question — you need to check on them quickly without clicking around. One shortcut and you are there. And when an agent comes back with a suggestion, **Ctrl+Shift+Enter** to accept. No hunting for buttons, no breaking your train of thought.

There are shortcuts for just about everything — switching between panes, toggling the plugin panel, spawning terminals, managing notes, navigating the file browser. It sounds small but when you are deep in a multi-project workflow with several agent sessions running, being able to navigate entirely from the keyboard keeps you in that focused state where real work happens. The moment you have to stop and think about *where* to click is the moment you lose momentum.

---

## What I Have Learned Along the Way

**AI works better with context, not just prompts.** Having the file browser, editor, terminal output, and agent sessions all visible at once means I am feeding Claude better context naturally. I am not copy-pasting between windows or trying to describe what I am looking at. The AI can see what I see.

**Multiple agents are not a gimmick.** I genuinely use two agent panes simultaneously — one building, one researching. While Claude is implementing a feature in the Build pane, I am asking questions about the next feature in the Research pane. It is not multitasking for the sake of it. It actually keeps the flow going.

**Session management matters more than I expected.** Being able to pin sessions, stop and restart agents without losing context, and have lazy-spawned panes that only activate when you look at them — these small things add up to a workspace that does not fight you. The lazy-spawning also means better machine resource management — agents are not eating up CPU and memory until you actually need them, and you can easily manage all of them through the "Agents" tool in each project. On top of that, there is a collapsible toolbar at the top that shows token usage broken down by session, by week, and by model — plus real-time memory and CPU usage of your Claude processes. It keeps you aware of what your AI sessions are actually costing and consuming without having to dig through dashboards or check external tools.

**The tool should get out of the way.** Every feature I removed was as important as every feature I added. If something was not earning its screen space or mental overhead, it went away. The workspace has to feel like an extension of how you think, not a dashboard you have to manage.

---

## What Is Next

frms is still evolving. There are always more edges to smooth, more real-world usage patterns to accommodate. The notes system will likely get nested categories. More AI tools will be added as they mature. I am continuing to explore how multiple AI sessions can work together more intelligently within the same workspace.

If this sounds like how you think about working with AI — not as a chatbot you visit, but as a workspace you live in — I would love for you to try it.

**[Fill out the frms Beta Request Form](https://forms.gle/PLACEHOLDER)** and installation instructions will be sent your way.

---

*frms is built with Rust and [Iced](https://iced.rs). No Electron, no web views, no tmux — all native UI.*
