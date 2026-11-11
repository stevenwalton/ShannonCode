Let's face it, Claude Code sucks.
That's not a bad thing, if we're being honest, all code sucks.
It was good first run, lots of things were learned, but we need a
better coding agent that meets the needs of engineers, especially those who are
accustomed to using the terminal.
This version will suck too!
But the goal is to create something that sucks less.
In the age of agentic coding is that there should be lower barriers to rewriting
code. Rewriting a program sucks, but there's a sunk cost fallacy. 
Momentum can push us forward, but also hold us back.

This doc will serve as the overall design doc for a new Claude Code. Since it is
the second iteration, we'll use last name :)
We'll first start with what is wrong with the existing implementation. After
that we'll talk about a vision of a better coding agent. This document serves as
the vision, not the full design doc.

There has been promise that AI agents will replace engineers, but I doubt that
will happen anytime soon. Even in a world where the agents do all the coding
there is still plenty of room for engineers. And that is where this vision
differs. Claude Code is presented as the professional interface, but what is
needed is a system that works with the engineer, not against them. A tool for
the terminally terminal needs to understand the needs and ways in which us
terminally terminal operate. The tool needs to understand why in a world of
fancy interfaces we have surrounded ourselves with the banality of text. The
answer to that is shockingly simple: customization.

Work the way you want.
Work *how* _you_ want.
Don't just settle for the defaults.
With any good tool you need to learn how to use the tool and you should also
reshape the tool to work with you.
If we don't let our tools be reshaped then we're all just going to be the same
and make the same mistakes.
That's not the right kind of philosophy for a ground breaking technology that
we're all still trying to figure out how to best use!

# What's Wrong With Claude Code?
Where to begin? If you can't name a few dozen things yourself, then I'm
wondering if you have even used it. A good engineer is always a little grumpy.
Frustrated with the lack of capabilities or features helps us identify what to
make. But a good engineer also builds.

## A Game Engine?
There's a [famous Twitter
Thread](https://x.com/trq212/status/2014051501786931427)
([xcancel](https://xcancel.com/trq212/status/2014051501786931427)) where 
Thariq Shihipar, from Anthropic, explains that Claude Code is designed like a
game engine. This has received a lot of
[critiques](https://www.youtube.com/watch?v=LvW1HTSLPEk)
There is no reason for any of this complexity here. Claude, today, is just an
interface that prints text to the screen. There's no need for a budget.
Terminals can write to the screen
[hundreds](https://mitchellh.com/writing/ghostty-and-useful-zig-patterns) or
[thousands](https://github.com/const-void/DOOM-fire-zig/) of times a second. a
"60 fps" budget is silly. 

But I'll give the team some credit. I'm sure there was a good decision for this.
I'm sure there was a bigger vision. But that's not where it ended up.

## The Pain of Panes
Claude looks kinda like this
```
                              ┌─────────────────┐
                              │┌───────────────┐│
                              ││    Claude's   ││
                              ││    Output     ││
                              │└───────────────┘│
                              │┌───────────────┐│
                              ││   Prompt Box  ││
                              │└───────────────┘│
                              └─────────────────┘
```
The problem is that these don't scroll independently.
This makes working with Claude absurdly painful. These two sections are
functionally independent of one another and should operate independent of one
another. That is an absolute basic for TUI based design!

While writing this program Claude changed this feature.
Hurray! It scrolls independently!
Boo! I can no longer scroll with my keys and have to scroll with my mouse!
Worse, if the prompt gets long enough that it reaches half the screen height it
starts scrolling off.
I've had mixed success with this, but hey, at least I can use `G` and `gg` from
`vim`, right?
And if I keep scrolling the cursor up I get the previous message.
Maybe sometime `O` will have consistent behavior :)


## Permissions
Have you ever found yourself in this position?
```
 Bash command

   gh pr create
   Create a pull request

 This command requires approval

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don’t ask again for: gh pr *
   3. No
```
These permissions are out of control! You probably want to blanket allow
something like `gh pr view` but not let Claude run `gh pr create` unchecked.
Claude is effectively asking you to approve much more permissive things than you
likely intend.

## It's Just Text
So why are there flickering issues and screen tearing? People use `tmux` to
ensure they have scrollback but... it is just text! None of these things should
be an issue. You should never open your editor (`<C-g>`) and lose your chat
history. You should never resize your windows and have your text all garbled.

## Stacked Prompts
Maybe one of the most infuriating issues is Claude's usage of stacks when it
requires user prompts. What insanity! You read the prompt, and you may hit enter
as a new prompts comes across your screen. This just sets the user up for
failure. You don't need to grind LeetCode to know that LIFO is an objectively
poor choice for this. Even those unfamiliar with the basics of computers would
design such a system through queues. 

## I Could Go On Really! There's tons of little things that are just...
annoying. We could go on forever about this, but let's talk about what Claude
*could* be. Claude Code was made with poor choices but momentum turns even small
fractures into canyons. There's a million little annoying things like placing
configs at `$HOME` and not `XDG_CONFIG_HOME` or even the fact that you can write
a new skill then have to restarting claude instead of just reloading the config.
There's weird and confusing error messages like "You've hit your monthly spend
limit" when you've only hit your session limits. These are unnecessary
inconveniences and are more illustrative of a lack of attention to detail and an
unfamiliarity with how these systems are built. 

# What Claude Can Be
This may not be your vision, but it is mine.
The key thing I want is something that is extendable and people can build upon.

## An Environment, Not a Product

The thing that makes computers such useful machines is that they aren't a
product so much as they are an environment. The computer (and smartphone[^0]) is
nothing without the program (app). You might wonder why people like me are
obsessed with using the terminal and using terminal based tools. The answer is
simple: I can make the tools work for me.

[^0]: WHICH IS JUST A COMPUTER!!!!

A master craftsman[^1] needs to know the tools that they use. They are an
extension of themselves. Go talk to any woodworker and they will tell you about
custom tools they've made or modifications they've done to tools they've bought.
You see this in tons of crafts where people work with their hands. Then talk to
those of us who are terminally terminal. You will see that that's why we go this
route. Sane defaults are great, but you can't make a product for everybody. But
what you can make for everybody is something that they can mold and turn into
their own thing. 

[^1]: I am certainly no master

Just look around you. Do you know anyone with the same computer or phone? No no,
not like that. You're thinking hardware. Look more closely. No one is going to
have identical themes, apps, configurations, or any number of things. We all end
up customizing our things. Similarly no two people have the exact same home!
Those differences are what defines us. 

You make sane defaults, but you allow people to shape it to be their own.

## The Interface
The current interface is terrible, but it takes the shape of something that
could be.
The interface should be customizable, as this is the thing I expect most people
would want to change.
But we should also provide some defaults, with customization in them.

### A Simple Interface
There's a common design language in many interfaces today. `tmux`, `vim`,
`emacs`, and even your (modern) terminal emulator know some simple design choices.
In all of these you can slice up your screen and create separate spaces.
In `ghostty` we have splits, in `tmux` and `vim` we have panes. This is the
natural layout for these things. A simple design: we have a top pane which shows
Claude's output and a bottom pane for our prompts. Want to resize them? No
problem! Want to swap them around? No problem! There should be nothing stopping
you from creating anything like this
```
 ┌─────────────────┐ ┌─────────────────┐┌─────────────────┐
 │┌───────────────┐│ │┌───────────────┐││┌──────┐ ┌──────┐│
 ││    Claude's   ││ ││   Prompt Box  ││││Claude│ │Prompt││
 ││    Output     ││ │└───────────────┘│││Output│ │ Box  ││
 │└───────────────┘│ │┌───────────────┐│││      │ │      ││
 │┌───────────────┐│ ││    Claude's   ││││      │ │      ││
 ││   Prompt Box  ││ ││    Output     ││││      │ │      ││
 │└───────────────┘│ │└───────────────┘││└──────┘ └──────┘│
 └─────────────────┘ └─────────────────┘└─────────────────┘
```
But I'm a `vim` user, and I'll tell you there's so much more you can do!
Not just panes, but tabs, buffers, oh my!
Today people open multiple terminals to run multiple instances of claude, but
these should be running in different tabs or buffers. `--resume`? You should be
able to cleanly resume from the program, jumping around as you please.

Importantly, the prompt box should allow you to use whatever `$EDITOR` you want.
You're a `vim` user, like me? Great! Use actual `vim` commands, and not these
half-assed controls[^4]. You're an `emacs` user? Great! Use that. Use `nano` for
all I care. However you type best is how you should be able to write. You are
just writing the payload, and there's no reason you shouldn't be able to write
that payload in the way you want.

[^4]: I always hear people tell me that tools like VS-Code have "`vim`
    keybindings". That is all bullshit. `vim` isn't about `hjkl`. `vim` isn't
    even about the motions. It is a text-***editor***. 

In reality, we should also be able to open a terminal and work ***WITH***
Claude. My terminal often ends up looking like this
```
┌─────────────────────────────┐
│┌────────────────┐ ┌────────┐│
││ ┌─────────────┐│ │        ││
││ │Claude's Text││ │        ││
││ └─────────────┘│ │        ││
││                │ │  CLI   ││
││   ┌───────────┐│ │        ││
││   │Prompt Box ││ │        ││
││   └───────────┘│ │        ││
│└────────────────┘ └────────┘│
└─────────────────────────────┘
```
Where I split `ghostty` in half, with Claude on the left and `zsh` on the right.
It isn't uncommon for me to split that right pane in half too, so I have `vim`
open. There I can read files while Claude is working away, review what is
written, run `grep` commands on my own, run my associated `git` and `gh`
commands, and all that. But right now these are not able to easily communicate.
Being a `vim` user the worst possible thing to do is lifting my hands away from
the keyboard and reaching for the... mouse! Instead, we should be able to select
blocks/lines and include them in Claude's context, so he can work ***with*** us
and not just *for* us. There needs to be a true pair-programming like experience
here.

Tool prompts may seem like a problem, but this is already something that TUIs
routinely handle. Even `vim` has popup windows. These create no barriers for a
simple interface.

### A True Fancy Interface
But there is a reason to have a fancy interface. There's modern terminals these
days that can do so much more. They support things like
[sixel](https://github.com/saitoha/libsixel) and the [kitty graphics
protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/), enabling them to
display high resolution images. If you think terminals can't do much, you
clearly haven't heard of [notcurses](https://github.com/dankamongmen/notcurses).
Anthropic talks about a budget of "60 frames per second" but if we wanted a
fancy terminal that could process pictures and all kinds of things we should be
expecting something on the order of hundreds or thousands of frames per second.
Just watch the [norcurses demo
video](https://www.youtube.com/watch?v=dcjkezf1ARY). Or check out [ghostty](https://ghostty.org/)
where Mitchell Hashimoto talks about
[Making a Terminal Emulator Really, Really Fast](https://www.youtube.com/watch?v=cPaGkEesw20&t=3015s).

Claude *could* be a full featured GUI-esque program running in the terminal if
we wanted to. Capable of displaying high resolution images and even play some
videos. With modern terminal emulators these things are actually possible. Maybe
this is the vision Thariq originally had, but that's not what ended up being
created.

In either case, we should be able to use *either* a simple TUI or a fancy one. 

## Better Orchestration
At the end of the day, creating an agent requires us to perform a magic trick.
We need to make a stateless machine appear stateful.
An agent is its history and harness. If you make the API calls with the same
history and harness then the results come out the same (well... the results
aren't deterministic, but they are identical up to that). 

### Walking Through A Conversation Graph
We can think of a lot of the agent though the view of a graph of chats. We can
create an analogy with how `git` works. The mental model isn't perfect, but it
can still be very useful.
```
┌──────────────────────────────────────────────────────────┐
│ Episode                                                  │
│    ┌───────────────┐ ┌───────────────┐ ┌────────────────┐│
│    │ Turn    ┌────┐│ │ Turn    ┌────┐│ │ Turn     ┌────┐││
│    │         │User││ │         │User││ │          │User│││
│    │         └────┘│ │         └────┘│ │          └────┘││
│    │        /      │\│        /      │\│         /      ││
│    │┌──────┐       │ │┌──────┐       │ │ ┌──────┐       ││
│    ││Claude│       │ ││Claude│       │ │ │Claude│       ││
│    │└──────┘       │ │└──────┘       │ │ └──────┘       ││
│    └───────────────┘ └───────────────┘ └────────────────┘│
└──────────────────────────────────────────────────────────┘
```
Each `Turn` is composed of two blocks of text: 1 from the user, one from the
agent. Multiple `Turns` compose an `Episode` or conversation.
In reality it doesn't have to be a user/agent interaction, but can be two
arbitrary entities. Similarly, an `Episode` can be `n` `Turn`s or 1.
But with this framework we should understand that it becomes easy to clone or
fork these agents, and that's where an analogy to `git` results in a useful
mental model.

This is a **stateful** machine and we should be able to use it as such. We can
walk through each state, be that the agent, subagent, or whatever. For normal
users the magic trick still succeeds, but for the magicians (powerusers) we can
pull back the curtain. We need to be able to do this to improve the 'magic
trick' or adapt it to new settings.

Taking on this view of a conversation graph also gives us far more capabilities.
You've probably had a conversation and wished you could pull in context from a
different session. 

The user should be able to easily visualize the graph and walk through each
node. With mutation it is easy to lose track of where the current state is. We
must provide tools for creating maps and understanding the underlying processes.

#### Greater Episode Manipulation
Continuing with the `git` analogy we start seeing how we might be able to do a
lot more things than simply fork or clone. We might want to branch off of a main
conversation, do some work, and then bring that conversation back into "main",
as it were. This can be done in parallel to an evolving conversation on main or
not. If we simply branch, work there, then move back, our merge strategy is
trivial, as it is identical to having just worked on main. But if we operate on
main and operate on our branch, we might want to do something like a compaction
(squash and merge) where we only summarize our actions from that side task.
There are going to be many ways to perform merge strategies here and we should
make this available for others to make and share, but these two provide a clean
starting place.

#### Better Subagents? A Tool for Powerusers.
If you're thinking about what we've said carefully you'll notice that this
creates a unique opportunity for subagents. Right now they are only single-turn.
The subagent effectively has the trivial merge strategy as the agent has a turn
with the subagent.

```
┌──────────────────────────────────────────────────────────────┐
│ Episode                                                      │
│    ┌───────────────┐  ┌─────────────────────────────────────┐│
│    │ Turn    ┌────┐│  │ "Turn"                        ┌────┐││
│    │         │User││  │                               │User│││
│    │         └────┘│  │                               └────┘││
│    │        /      │\ │ ┌────────────────────────────┐      ││
│    │┌──────┐       │  │ │┌──────┐            ┌──────┐│      ││
│    ││Claude│       │  │ ││Claude│            │Claude││      ││
│    │└──────┘       │  │ │└──────┘            └──────┘│      ││
│    └───────────────┘  │ │        \          /        │      ││
│                       │ │         ┌────────┐         │      ││
│                       │ │         │Subagent│         │      ││
│                       │ │         └────────┘         │      ││
│                       │ └────────────────────────────┘      ││
│                       └─────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────┘
```
We can see the interaction looking a lot more like this. You can extend this
mental model to see how parallel subagents work and all that.

Where this mental model helps is that we can start to imagine multi-turn
subagents working in the background. And since they are the history and the
harness, we should have more control over them. Maybe we want agents that are
calling different models. Maybe we want to adjust the parameters like
temperature, the thinking level, top_k, logprobs, or
[others](https://platform.claude.com/docs/en/api/messages/create)[^3].


[^3]: I realize Anthropic is deprecating parameters like top_k and temperature,
    but these are just illustrative.

The point is that we should be able to get full access and play around here. We
should be able to modify the history, walk through and inspect, and get full
control. Powerusers will create new things and show us new ways to do things,
while most of the users can just use what already exists. The platform needs to
enable people to experiment. Even though most people won't be powerusers, the
powerusers are the ones that shape the environment for everyone else. [Just
because the group is small doesn't mean it is not
important.](https://www.youtube.com/watch?v=5zUndMfMInc)

### Merge Strategies
With the effective ability to be able to control our own subagents (and
arbitrary depth) we have to be able to control how to merge these back.
We will create some basic merge strategies but we also want to be able to enable
users to define their own.

#### Straight Merge
This is the simple merge strategies. You branch, work on "a feature" then merge
that branch into main while nothing has touched main in the mean time. Simple
and straight forward. A baseline.

#### Squashing/Summarization
Same as before, but them we can compact the information from the branch. This is
going to be extremely similar to the current way subagents are used in Claude
Code. The parent agent just sees the final return message. We extend this a
little so that the agent can summarize a selected set of turns (via picking or
through a range) and/or a user manually edit. 
Summarization can be handled by passing the branch to the agent and asking it to
distill the information within it.

#### Cherry-Pick
Not all nodes contain relevant information that need to be preserved.
Cherry-pick your way to a simple merge or distill based on selected nodes.

#### Roll Your Own!
Everything in here is about customization. If you want to create your own
merging strategy, you can build it in too! We create the environment so you can
do whatever crazy shit you want. Manipulate that conversation any way you
please.


### Agent & Subagent Scheduling
OpenClaw gained popularity by connecting Claude to a cron job. We can do so much
more than that. Cron is easy to support, but cron is limited. We support a wide
range of triggers. Can provide support for `launchctl` based triggers for OSX
and `systemd` based triggers on linux. Schedule tasks by calendar, path
modifications, `inotify`, sockets, device mounting/unmounting, other claude
tasks, and however you want. 

## Full Agent Access
`<C-o>` to see agent details? IDK about you but usually `<C-o>` doesn't do shit
for me. *From the terminal* people should be able to gain full access to the
agent and everything. If you want to inspect, you should be able to inspect!
Plainly and easily, without leaving the interface. They should be able to, in
the CLI, open up memories, read the raw API calls, edit skills and MCPs, or any
of that. 

## Better Customization
The point is to build an environment in which they can make their agents work
for them in the way that they want to work.

### Plugins
People should really be able to build. We don't know what people will do, but
whatever it is, we want to make it easy for them to do that.

### Paths
One aspect of Claude Code that's quite annoying is dealing with the permissions
of what files and programs it has access to. There's already a clean solution to
this in the unix world: environment variables.
Claude should have its own `$PATH` variable to allow it to know what programs it
can run. This isn't a sandbox, but it does make a cleaner approach to tool
limitation. Don't want Claude to run `rm` commands? Don't have `rm` available in
its path. This doesn't create an actual sandbox like security system and Claude
is still running as your user[^2], but it is another form of control.

[^2]: Ideally, in the future, Claude would be running as a separate user on your
    device. With its own permissions. Its own `uid`, environment, home
    directory, and everything. Still not a full sandbox, but better than full
    access.

### Permissions
Right now Claude suggests to you permissions and you either have to take it or
leave it. No chance to edit. That's not a tool you work *with*, that's a tool
that encourages you to act recklessly. Claude Code wants you to blindly accept
what it suggests. When people are unfamiliar with linux tools this leads to
unintended behaviors. People see commands like `find`, `xargs`, `awk`, `sed`,
`tar`, and many others thinking that they are specific tools and can't run
arbitrary commands. 

`find` is one of my favorite tools, but it isn't only used
for searching for things. You can see in [my own
dotfiles](https://github.com/stevenwalton/.dotfiles/tree/master/rc_files) how I
use `find` to execute arbitrary commands. That's what makes it so useful, but
also what makes it dangerous. You probably want to allow `find` to be used
freely by Claude, *when it is being used to find files*, but not when `find`
calls `-exec`. How many people even know about `-fls` or `-ok`? 

Bash is a [terrible language](https://mywiki.wooledge.org/BashPitfalls), yet I
love it. But with these types of tools we need the capacity to have more refined
permissions. Allow arbitrary usage. Block certain flags. Allow Claude, in a
`/btw`-like manner, explain to you what the tools do and how to properly build
safe harnesses. You shouldn't be hitting `enter` on your keyboard all the time
and neither should you be running `--dangerously-skip-permissions`. The reason
people run the latter is because the permission system was not properly
designed. A well designed system should encourage people to use your tools
safely and efficiently, not circumvent them.

## More Information
Claude Code has commands like `/status` that are useful, but they are not that
useful. `/usage` will show you the tokens and usage you've used for that
session, but that's it. If you want to know if you're approaching your monthly
limit you have to open up the GUI or go to the website. These kinds of things
are a bit insane. Everything should be available in the app, at your fingertips.

And I mean nothing. You want to view the raw API calls? Go ahead! Whatever
information is exposed through the API is available to you. We won't put it
front and center (unless you configure it that way) but it's available for you
to inspect and edit.

## Tool Use
First and foremost, tool use prompts should come in queues. None of that
bullshit where a new prompt can overlay your existing prompt. No accidental
permissions by approving the wrong thing. Prompts always land in a queue.
In your settings you can set all tools to have claude have auto access, require
human approval, require the human to run, or fully block. `[auto, approve,
manual, block]`. By default all tools come with `approve` permission.

### Improving on Existing Tools
#### Timeouts
Configure that shit. You choose how long it stays, can be forever if
you want (default) or timeout right away. No one is forcing anything on you.

#### Tool Permissions
The permissions? Don't just let claude suggest globs to you, you can suggest
globs to it! If it asks you for `gh pr *` you can say "fuck no, but I'll give
you `gh pr view *` and that permission should persist. Claude gives you a weird
command? Go ahead and edit it! You're working together and sometimes the agent
is just guessing. Directly edit the commands, just like as if you were pair
programming. Sometimes it is just easier to type that flag in for that junior
engineer or delete those unnecessary lines when it tries to run `git -C $CWD`.

#### Compaction
Get access to the compaction message, vet it, edit it, accept as is. Claude can
make the suggestion, but you get to inspect and edit. Default stays as you're
used to but edit a config and it'll automatically drop you into the edit for the
message. Or run `/compact --edit` or `/compact -e` instead and get it as a
one-time behavior (complement is `/compact -y`/`/compact --auto-approve`).
Configure to let claude compact themselves. That way you can give instructions
like "we're going to do this task, then compact" and it can act right away
without approval (option is configurable).


#### Plan
Be able to inspect the plan better, open it in `$EDITOR`, discuss parts of it
with Claude. Discuss in a side channel so you aren't modifying the main context.
Be able to inspect files it wants to change and have conversations. We can
compact merge this side conversation so we don't eat up context nor introduce a
lot of irrelevant information.

## Memories
Memories are the lifeline. They persist through sessions, but can get messy.
If you aren't reading these, you aren't properly working with your LLM. They
have no sense of time and things go stale quickly. Make it easy to read these.
Editing automatically loads the altered versions into the conversation. 

But memories shouldn't be locked to projects directories. They go wherever you
want! Organize as you please. Default to a `$XDG_CONFIG_HOME/shannon/project/`
location. Load those or don't! Have a shared location that multiple agents can
have a collective memory. Shard your memories if you want. We'll provide some
sane defaults but use your config (your global or your project's) to modify as
you want. You can organize however you want. Hell, you can let Claude organize
however they want!

## Actual Documentation
How the fuck do I use this agent? Don't go hunting on some website, just type
`:help` (or `:help <foo>`). See everything you can do. Read it or point Claude
to it so Claude can tell you. Either way, you shouldn't have to leave the
program to go find help. Everything should be at your fingertips.

# Some Example Workflows
To help with the vision, we might want to think about some example workflows.

## Researcher: Model Variance
These models are non-deterministic, so their responses can vary. A useful thing
for researchers to do is send the same prompt multiple times upstream to the
provider. With our framework, we've effectively cloned the agent.

Being able to fork/branch provides greater utility too, as we can start to see
how that variance increases as conversation length increases. Keep "one" agent
until turn N and then make 5 clones and look at the results. This makes
capturing variance much easier for researchers.
```
┌───────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐
│ Response 1│ │Response 2│ │   ....   │ │Response N│
│           │ │          │ │          │ │          │
└───────────┘ └──────────┘ └──────────┘ └──────────┘
┌──────────────────────────────────────────────────┐
│                 Prompt Box                       │
└──────────────────────────────────────────────────┘
```

## Researcher: Model Comparison
If APIs allow for other models then this task greatly extends. You don't just
send the same prompt (or history) to one model, but however many you want! 

## Engineer: Orchestration
Every company wants its own orchestrator, and this type of tool enables them to
build on top of it and do what they want. This is much more powerful than the
SDK. All they have to do is build their harness and whatever UI they want to do
on top of it. 

## Engineer: Pair Programming
Truth is that agents shouldn't write all the code. Even if they could, this
would not be affordable for every user. Tokens are expensive and there's a lot
that smart engineers could do that would allow greatly reduce tokens, while also
reducing the junk data that fills the agent's context. Being able to pass
information back and forth like bash command results or snippets of code should
be easy with a proper agentic coding platform. The engineer wants to treat
Claude as a peer, not a pure replacement.

Similarly, they want to be able to better see what Claude is doing. Claude Code
makes this hard. We should easily be able to open the files that Claude does
(with `$EDITOR`), jump into the raw API calls, being able to edit
calls/memory/etc right in place without leaving the agentic coding platform. 

## General User: Importing and Exporting Conversations
I can't be the only one that works on more than one computer.
You go from one to another and ops, Claude is gone.
But why can't I import from one computer to another, easily?
Have no fear, you no longer need to `rsync -azchP
~/.claude/Projects/-Users-yourname-some-random-ass-directory/
remote_machine:~/.claude/Projects/-home-yourname-some-other-random-ass-directory/`!
Not only that, but let's fucking format the `jsonl` files so that they are
actually readable!

## General User: Brain Surgery
Anyone using enough Claude (or any LLM) has had it hallucinate on you and those
hallucinations infect Claude's memory.
In work we had a fake person named "Steve" and after a few compactions Claude
decided my name was
[Steve](https://cloudywithachanceofmeatballs.fandom.com/wiki/Steve).
Now Claude's memories are all messed up, with files having both "Steven" and
"Steve" in them.
Including a file that says "User is Steven, not Steve".
Sometimes you need to go in and perform brain surgery.

These files should be easy to bring up and inspectable.
We got to help our LLM friends out!
They have a hard time telling you how many r's there are in strawberries,
they're going to struggle with differences like that.
When left unresolved it only causes them to make more hallucinations.
So editing memories is part of the general work.
We should make this easy and straight forward, or else people won't do this
common task.

Edit one of these files? Why reload Claude?
It's a new message, we can just reload the files.
Messes up caching?
Sorry, that's on Anthropic.
If they gave us a means to mark what was cached and what is new, then this would
be resolvable.
But if you're on the 5min cache, this won't likely matter much anyways.
While we're at it, edit whatever you want and reload without restarting.
System prompts? Editing history? Editing Skills? Editing Agents? Editing MCPs?
Typing `/exit` + `claude --resume` is too much work and unnecessary.

## General Use: Where Are You?
In a long Claude session?
The task growing larger than expected?
Oh shit! Claude compacted and I wasn't ready! Now Claude thinks I'm a
[monkey](https://cloudywithachanceofmeatballs.fandom.com/wiki/Steve)!
This could be easily solved with some clear indication of your current context
size.
You made a game engine but not a progress bar?!

Not only that, but how far back was something in your context?
There's a recentcy bias, so are you counting your tokens while you scroll back?
Easily solved with a little heatmap to the side.
Or wherever you please.
Hell, want to apply a heatmap to the text?
Go ahead. I'm not gonna stop you! I'm not a cop!

Know where you are in the conversation.
Don't be thrown off by surprise compactions.
Hell, you should even be able to edit and work with the compaction.
To see what Claude wrote!
The whole point is we're trying to make the LLMs work with you.
Not against you.
Not *for* you.
And certainly not having you work for the LLM.
Do what you want, and how you want it, even if that is just the default
settings.
