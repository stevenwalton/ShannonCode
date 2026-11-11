_Shannon Code is an experiment, and still in early development.
Issues, suggestions, complaints, PRs, and insults are all welcome._

<h1 align="center">Shannon Code</h1>

<p align="center"><em>Claude? I think you mean Dr. Shannon.</em></p>

<p align="center">
A coding-agent harness for the terminal that works <strong>with</strong> you,
not <strong>for</strong> you.
</p>

<table align="center">
    <tr>
        <td>
            <img width=800px src="assets/images/ShannonCover-dark.png#gh-dark-mode-only" alt="A conversation with Shannon" />
            <img width=800px src="assets/images/ShannonCover-light.png#gh-light-mode-only" alt="A conversation with Shannon" />
            <br>
            <p align="center">Using Shannon</p>
        </td>
    </tr>
</table>

Most harnesses are built so the agent does the work while you watch.
You approve everything, or you approve nothing and hope.
Shannon is built for the space in between: you decide how much the agent does
on its own, down to a single flag of a single command, and you can see, edit,
and rewind everything it did.

Shannon is made for people who live in the terminal.
If you have to take your hands off the keyboard, please submit a bug report.

# Why Shannon?

### Permissions that mean what you meant
"Yes, and don't ask again for `find *`" is how an agent ends up running
`find . -delete` and deleting your whole project[^0].
Shannon reads a command the way your shell does (pipelines, redirects,
substitutions, `cd`s, wrappers like `sudo` and `timeout`) and decides each
piece on its own.
So you can say what you actually mean, typed in a session or kept in your
config:

[^0]: Something similar to this actually happened while developing Shannon! Even
    `git commit` can be dangerous!

```
:allow find *
:deny find -delete
:deny find -exec
:allow :read src/**
:deny :write .env
:allow re:^cargo (build|test|clippy)( |$)
```

Rules can name a program and its flags, a path and what's done to it (read,
write, delete), or a host a command reaches.
When the simple notation isn't enough, a rule can be a regular expression.
When a call asks, `A` proposes the narrowest rule that would have allowed it,
and you edit it before it's kept.
You can also edit the command itself before it runs, and the agent is told
what actually ran.

<table align="center">
    <tr>
        <td>
            <img width=800px src="assets/images/Decision-dark.png#gh-dark-mode-only" alt="The decision pane" />
            <img width=800px src="assets/images/Decision-light.png#gh-light-mode-only" alt="The decision pane" />
            <br>
            <p align="center">Helping you to decide</p>
        </td>
    </tr>
</table>


<details>
<summary>You can even edit them!</summary>
<table align="center">
    <tr>
        <td>
            <img width=800px src="assets/images/EditCommand-dark.png#gh-dark-mode-only" alt="Editing a command" />
            <img width=800px src="assets/images/EditCommand-light.png#gh-light-mode-only" alt="Editing a command" />
            <br>
            <p align="center">Help the agent and edit the commands it wants
            to run</p>
        </td>
    </tr>
</table>
</details>

### Built for the terminally terminal
Shannon is a `vim`-like TUI built on [ratatui](https://github.com/ratatui/ratatui).
We work with text so we need a tool that's good at editing text.
You keep your full history.
No flickering.
No loss of conversations when resuming or compacting.
Smooth scrollback.
Resize your terminal without losing text.
Shannon is closer to a text editor than to a game engine.

Some editing features:
- Motions, operators, text objects, Visual mode, and search, in the prompt
  and in the transcript; marks and folds in the transcript.
- Panes, buffers, and tabs: `<C-w>v`, `:vsp`, `:ls`, `:tabnew`.
- `]]` and `[[` jump between your prompts.
- Yanks reach your system clipboard (OSC 52, over ssh, and inside tmux with
  `set -g allow-passthrough on`).
- `<C-x><C-e>` writes your prompt in `$EDITOR`.
- The mouse works, but you can turn it off.
- A manual in vim's style: `:help`, with links you follow with `<C-]>`.

### Your conversations are a graph
Every conversation is a graph of turns, and `:graph` lets you walk through it.
Automatically branch by loading a turn and continuing the conversation.
Merge them.
Squash them.
Go back to any turn and continue from there, and Shannon branches for you.
Be a revisionist and edit the history itself, even the agent's replies.
Do whatever you want[^1].
We don't know your needs, so our goal is to empower you.

[^1]: Shannon doesn't care what you do, but make sure you don't upset your model
    provider. (Shannon currently only supports Claude)


Name sessions so you can find them again.
Easily explore your conversations because you forgot which conversation was
actually the one you wanted.
See your token usage and metadata.

<table align="center">
    <tr>
        <td>
            <img width=800px src="assets/images/graph-dark.png#gh-dark-mode-only" alt="The conversation graph and its inspector" />
            <img width=800px src="assets/images/graph-light.png#gh-light-mode-only" alt="The conversation graph and its inspector" />
            <br>
            <p align="center"><code>:graph</code>: conversations, turns, a fork, and the inspector</p>
        </td>
    </tr>
</table>

Already have Claude Code conversations in your directory?
`:graph` lists this directory's, and `:import` brings one in as a Shannon
session you can continue with any model.

### Keep it secret, keep it safe, spend it wisely
Use `:secret`/`:alias` to define variables that the agent can use but not read
the values of. Protect your keys from the agent, the model provider, or anyone
reading your logs. The header image shows what the agent sees
(`{{secret.b4a9}}`) and what you see (`hunter2`). If we see something that's
obviously a password we'll try to mark it for you.

### Multi-agent conversations
Organize your conversations as you please. Split your windows into multiple
panes. Place different work in different tabs.
This is *your* workspace and we won't tell you how to organize.
We don't even care if you can see the agent's chat. Use `:Send <ID>` to send
your prompt to any active agent.

Want to send the same message to 100 different agents? Load them up and `:Send
ball`. Or send to only the ones on screen with `:Send all`.

Fork a session and run your two branches side by side (`:fork`/`:vfork`).


### Yours to shape
You can't make a product for everybody, so our goal is to make an environment
you can modify to your needs.
Sane defaults, then everything is configurable.
Configs under `$XDG_CONFIG_HOME/shannon/` (no cluttering your home!) hold only
what you changed, and `:set` changes things for this run.
You can `:source` other files if you want to organize differently.

Select from themes to match your style.
~Blinding~Light mode? Dark mode? We even adjust to your GitHub theme.
Change the status line.
Remap keys.
Change alerts.
Export your settings.
Change your settings per project.
We want to make it easy for you.

# Getting started
Shannon builds from source on macOS and Linux with Rust 1.88 or newer
(`rustc --version`; [rustup](https://rustup.rs) installs it).

```sh
git clone https://github.com/stevenwalton/ShannonCode.git
cd ShannonCode
cargo install --locked --path crates/shannon-tui
```

`--locked` builds with the exact dependency versions Shannon is tested with.
The `shannon` binary lands in `~/.cargo/bin`, which needs to be on your `PATH`.

### Jump in with the tutor
```sh
shannon tutor
```
Inspired by `vimtutor` we'll teach you how to use Shannon, even if you don't
know `vim`.
You can try Shannon without spending a single token.
No API key or subscription required.
The tutor uses a fake model (`stub`) by default and runs in a temporary
directory.
It is hands on to help you be effective with your agents.

<table align="center">
    <tr>
        <td>
            <img width=800px src="assets/images/tutor-dark.png#gh-dark-mode-only" alt="shannon tutor" />
            <img width=800px src="assets/images/tutor-light.png#gh-light-mode-only" alt="shannon tutor" />
            <br>
            <p align="center"><code>shannon tutor</code></p>
        </td>
    </tr>
</table>

### Setup and choose a model
```sh
shannon setup
```
(or `:setup` while running Shannon).

Three ways to run:

| Backend | What you need | What you get |
|---|---|---|
| Anthropic API | An API key from the [Claude Console](https://console.anthropic.com) | Full featured. Use any model your key allows. |
| Claude Code | Your own installed `claude`, signed in | Shannon's interface, graph, and permission gate over Claude Code's agent and tools |
| Stub | Nothing | A pretend agent, for learning the interface |

Shannon never asks for your Claude.ai login.
It uses an API key, or runs your own `claude` (`claude -p`) as it is.

# A brief look

| You want to | Type |
|---|---|
| Send a prompt | type in the prompt, `<Enter>` in Normal mode |
| Read the transcript like a buffer | `<C-w>w`, then any motion |
| Open a second session beside this one | `:vsplit` then `:new` (`:vnew` for both), or `:fork` to clone this one |
| See and walk every conversation | `:graph`, or `shannon --resume` at launch |
| Answer a command that asks | `a` allow once, `d` deny, `e` edit, `A` add a rule |
| See the rules in force | `:rules` |
| Keep a value from the agent | `:secret <value>`; `:alias` lists them |
| Change model or effort | `:model`, `:models`, `:effort` |
| Change anything else | `:set`, `:config edit`, `:themes` |
| Read the manual | `:help`, `:help <topic>` |


# Goals
Shannon started from frustration with the harnesses we have, and from a
belief that a coding agent should be an environment you shape, not a product
you put up with.
I'm still frustrated, but at least less so now and I can do something about it!

- **You choose the level of control,** per command, per flag, per path.
  Nothing is all-or-nothing.
- **Nothing is hidden.** You can see what the agent was told, what it ran,
  what it cost, and you can edit any of it.
- **History is data.** Conversations are graphs you can branch, merge, and
  rewrite, not a scrollback you lose.
- **The terminal is home.** Keyboard first, vim's conventions, plain text,
  your editor, your colors.
- **Defaults, then freedom.** Every behavior has a sane default and a way
  to change it.

### What's not there yet
- Plugins: the strategies and tables are built to be extended, but there's no
  plugin interface *yet*.
- Password-protected sessions: planned; today the setting changes nothing.
- Shannon's permission gate is not a sandbox.
  It reads and decides on what the agent asks to run, which is a strong
  guard against an over-eager agent, but it's no substitute for running
  untrusted work in a container.
- Models: Anthropic's, directly or through Claude Code.

Shannon is still in early development and there's a lot of ideas that still need
to be created. Submit your own or contribute.

# Documentation
- `shannon tutor`: a hands-on tour.
- `:help`: the full manual, right inside Shannon.
- [`docs/DesignDocs/`](docs/DesignDocs/): how and why things work the way they
  do, starting from the original ~rant~[`vision.md`](docs/DesignDocs/vision.md).

# FAQ
## Why "Shannon"?
Claude shares a first name with Claude Shannon, who gave us information
theory.
Anthropic won't say if Claude is named after Claude Shannon, but we will.

## Why not use Claude Code, Pi, cmux, etc
Why can't a guy just do things his own way?

I didn't find a harness that achieved the goals I actually wanted.
And hey, there's no better way to learn how to do things than to do it
yourself[^2].

[^2]: With some help from Claude

# Contributing
[`CONTRIBUTING.md`](CONTRIBUTING.md) says how to build and check a change.

# Security
Found a way around the permission gate, a secret reaching the model, or another
security issue?
Please report it privately, as [`SECURITY.md`](SECURITY.md) says, not in a
public issue.

# License
MIT: see [`LICENSE`](LICENSE).

Shannon is an independent project.
It is not affiliated with or endorsed by Anthropic.
Claude and Claude Code are trademarks of Anthropic.
Shannon has no relationship or affiliation with Dr Claude Shannon.
All sarcasm and opinions expressed are solely those of my cat.
