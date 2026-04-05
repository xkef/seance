# Mitchell Hashimoto on libghostty Ecosystem & Multiplexing

Collected 2026-04-04. Direct quotes from Mitchell Hashimoto about his vision
for libghostty as a foundation for others to build on, with multiplexers as
a primary use case.

## Multiplexers on libghostty

Changelog interview #622 (https://changelog.com/podcast/622):

> "there should be a multiplexer like TMUX where the core is just libGhostty,
> and you just focus then on the stuff above it."

> "if LibGhostty detects that it's running within Ghostty, it could just
> stop...you no longer pay for that anymore."

## Enabling others to build on top

Changelog #622:

> "I'm really trying to build this cross-platform artifact, this library that
> you could build terminal emulator applications on top of."

> "I don't want to build an iOS application, an Android application, or
> whatever future platforms exist. I want to enable others to do that."

> "the real goal with Ghostty is impact."

(Chose MIT licensing to "enable that impact no matter what.")

## libghostty is bigger than Ghostty

Blog post "Libghostty Is Coming" (https://mitchellh.com/writing/libghostty-is-coming):

> "libghostty is the next frontier for Ghostty and I think it has the ability
> to make a far larger impact than Ghostty can as a standalone application
> itself."

> "terminal multiplexers like tmux or zellij are also full terminal emulators!
> Editors embed their own terminal emulators too..."

> "we have some community members working on other libghostty-consuming
> projects, but we could use as many as we can get!"

## Ecosystem of diverse terminal apps

About page (https://ghostty.org/docs/about):

> "Ghostty the project also aims to enable other terminal emulator projects to
> be built on top of a shared core. This allows for a more diverse ecosystem
> of terminal emulators that can focus on higher-level features and UIs without
> needing to reimplement the core terminal emulation."

## Future libghostty-<x> libraries (renderer, input, widgets)

Blog post "Libghostty Is Coming":

> "Longer term, we will provide more libghostty-<x> libs that expose
> additional functionality such as input handling (keyboard encoding is a big
> one), GPU rendering (provide us with an OpenGL or Metal surface and we'll
> take care of the rest), GTK widgets and Swift frameworks that handle the
> entire terminal view, and more."

> "These will be structured as a family of libraries to minimize dependency
> requirements, code size, and overall maintenance complexity."

## Sources

- Blog: https://mitchellh.com/writing/libghostty-is-coming
- Changelog #622: https://changelog.com/podcast/622
- About: https://ghostty.org/docs/about
- Mastodon: https://hachyderm.io/@mitchellh/115249608385563525
- X: https://x.com/mitchellh/status/1970208215259607436
- X (re tmux): https://x.com/mitchellh/status/1875013250154406228
