# In-game UI for plugins — a design

Nick asked (2026-09-27) for a UI framework plugins can use: sorting rules,
capturing a preferred inventory layout, toggling no-pause-on-focus-loss, "and I'm
sure many other things too".

This proposes **defining the plugin-facing API before building any renderer**, and
shipping the three named use cases without a renderer at all.

## What the game actually is

Measured from `NMS.exe`'s import table, not assumed:

    vulkan-1.dll        NMS is a VULKAN game
    sl.interposer.dll   NVIDIA Streamline -- DLSS, and frame generation
    XINPUT9_1_0.dll     a static import, which is why our proxy DLL loads at all

`d3d12.dll` and `dxgi.dll` show up in the loaded-module list but are **not**
imports: they arrive with Streamline and the Steam overlay. Anyone who checks the
module list instead of the import table will conclude this is a D3D12 game and
build the wrong thing.

Two consequences:

- **Frame generation means frames the game never drew.** An overlay composited at
  the wrong layer either vanishes on generated frames or smears across them. This
  is not a reason to avoid an overlay, but it is a reason not to hand-roll a
  `vkQueuePresentKHR` detour as the first attempt.
- **`SteamOverlayVulkanLayer64.dll` is already in this process.** Valve solved the
  same problem here with an *implicit Vulkan layer*, which is a documented,
  supported extension point rather than a detour. That is the precedent to follow.

## The three use cases, decomposed

Take Nick's own examples and ask what each actually requires:

| use case | really needs | UI? |
|---|---|---|
| toggle no-pause on/off | a hotkey, and confirmation it happened | a line of text |
| capture current inventory layout | a hotkey, and "captured 3 inventories" | a line of text |
| choose which sort rule set is active | pick one of a short list | a list, or a hotkey that cycles |
| **author** a sort rule | a real editor: text, drag, undo | not in-game |

Only the last one wants a rich UI, and it is the one that should not be in-game at
all. Authoring rules with a controller while a Sentinel shoots at you is worse in
every way than authoring them in the desktop app, which already exists, already has
a component library, and already talks to the hook over a named pipe
(`engine::pipe`).

So: **in-game UI is for acting and confirming; the desktop app is for authoring.**

## Four tiers, and only one of them is hard

**Tier 0 — hotkeys.** The message pump is already ours: `nopause.cpp` rewrites
`WM_ACTIVATEAPP`/`WM_ACTIVATE`/`WM_KILLFOCUS` in `PeekMessage`. A hotkey service
lives in exactly the same place. We also proxy `XInput9_1_0`, so a controller
chord is available too, and — more useful — we can *suppress* input to the game
while a plugin is capturing one. Almost nothing else has that for free.

**Tier 1 — transient text.** A toast: a line or two, a couple of seconds, no
input. This covers three of the four rows above completely. It needs somewhere to
draw, which is the one piece of real work, but a toast can tolerate being a frame
late or missing a generated frame in a way an interactive panel cannot.

**Tier 2 — an interactive panel.** Immediate-mode, keyboard and controller driven.
Real work: a Vulkan layer, a font atlas, input capture and — the genuinely hard
part — giving input *back* to the game cleanly. Defer until a plugin needs it.

**Tier 3 — the desktop app.** Rich authoring. Already built. A plugin declares the
settings it has; the app renders them; the values arrive over the pipe.

## The API to define now

The point of writing this before any renderer exists: **plugins should not know
which tier drew them.** An immediate-mode, draw-agnostic API lets the same plugin
code render as a toast today, a panel later, and a page in the desktop app — and
lets us change our mind about Vulkan without touching a single plugin.

Sketch, in the spirit of the opt-in-export contract already chosen (see
`reference-nms-prior-art` — a plugin exports `OnStart`, the host probes for further
capabilities with `GetProcAddress`, so adding one never breaks an existing plugin):

    // The host calls this once per frame, if the plugin exports it.
    void OnUi(NmsUi* ui);

    struct NmsUi {
        // Output. The host decides whether this is a toast or a panel row.
        void (*label)(NmsUi*, const char* text);
        void (*status)(NmsUi*, const char* text, float seconds);

        // Input. Returns true on the frame the value changed.
        bool (*toggle)(NmsUi*, const char* id, bool* value);
        bool (*choice)(NmsUi*, const char* id, const char** options, int n, int* index);
        bool (*button)(NmsUi*, const char* id, const char* text);

        // What the host is willing to draw right now, so a plugin can degrade
        // rather than assume. A plugin that only ever calls status() works even
        // when this is Toast.
        enum { Hidden, Toast, Panel } level;
    };

And separately, because it is not UI and should not be entangled with it:

    bool (*bind_hotkey)(const char* id, const char* default_chord);
    bool (*hotkey_fired)(const char* id);

Three properties worth keeping:

- **No handles, no retained tree.** A plugin that crashes mid-frame cannot corrupt
  a widget hierarchy that does not exist.
- **Ids are strings the plugin owns.** The same id addresses the same setting in
  the panel and in the desktop app, so tier 3 comes almost free.
- **`level` is honest.** A plugin asks what it can have and adapts, instead of
  drawing into nothing.

## Recommended order

1. **Tier 0 hotkeys**, plus `status()` writing to the existing log. Every use case
   above becomes *testable* immediately, with no renderer. Hotkey to toggle
   no-pause; hotkey to capture a layout; confirmation in the log.
2. **Tier 3 settings over the pipe**, so rule authoring lands in the app where it
   belongs.
3. **Tier 1 toast**, as a Vulkan implicit layer. Judge the layer approach on a
   toast before betting a panel on it.
4. **Tier 2 panel**, only when a plugin genuinely cannot be served by 1–3.

Steps 1 and 2 need no graphics work at all and deliver everything Nick named
except rule *authoring*, which step 2 covers better than any in-game panel would.

## Risks worth stating

- **Input hand-back.** Taking input from the game is easy; returning it so the game
  does not think a key is still held is where this class of overlay usually breaks.
  Tier 2 should be prototyped against that problem first, not last.
- **Frame generation.** Verify a toast survives DLSS-G before assuming a panel will.
- **The layer applies to a process, not to us.** An implicit Vulkan layer is
  registered by executable name; getting that registration wrong affects other
  Vulkan applications on the machine. Scope it tightly and make uninstalling it
  part of the feature, not an afterthought.
- **Anything drawn over the game is a support burden.** Screenshots of our overlay
  will be attached to bug reports about the game. A visible, obvious toggle to turn
  all plugin UI off is not optional.
