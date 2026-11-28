# Slicer and macros

Sailhand works with any Klipper printer as it is. A few settings in the slicer and a few common macros let it show
and offer more.

## Slicer settings

**Thumbnails.** Sailhand shows the slicer's thumbnail in the file list and on the print page, in a square. Of the
thumbnails in a file it takes the sharpest one in that square (the longest short side; on a tie the squarer one), so a
square PNG fills it without borders. A small one for the list and a square one for the print page are enough, for
example `32x32/PNG, 300x300/PNG` in the thumbnail setting of PrusaSlicer or OrcaSlicer. A file without any thumbnail
shows only its name in the list and a file symbol on the print page.

**Layers.** Klipper only knows the layers when the slicer tells it. With these lines the print page shows the current
layer and the number of layers as Klipper reports them, and the [layer pause](#pause-at-a-layer) becomes available.
Without them it shows no current layer, and the number of layers only as an estimate from the object and layer height
(with a `~`). The heights above the layers need no setup: the current one is the Z of the file
(`gcode_move.gcode_position`, without Z offset and bed mesh, the lowest of the last two seconds so a Z hop does not
count; it stays during a pause), the maximum is the object height of the file. In PrusaSlicer, SuperSlicer and OrcaSlicer (Printer settings → Custom G-code):

| Field                    | Line                                                  |
| ------------------------ | ----------------------------------------------------- |
| Start G-code (first line) | `SET_PRINT_STATS_INFO TOTAL_LAYER=[total_layer_count]` |
| After layer change G-code | `SET_PRINT_STATS_INFO CURRENT_LAYER={layer_num + 1}`   |

Other slicers use other placeholders; the [Mainsail](https://docs.mainsail.xyz/slicers/prusaslicer/) and
[KlipperScreen](https://klipperscreen.readthedocs.io/en/latest/Quicktips/) documentation list them.

**Objects.** To exclude single objects during a print, Klipper needs `[exclude_object]` in `printer.cfg` and the file
needs the objects labelled: turn on the slicer's option to label objects for Klipper (it writes
`EXCLUDE_OBJECT_DEFINE`), or let Moonraker do it with `enable_object_processing: True` in its `[file_manager]` section.
When the file defines the outlines of the objects, the Objects dialog draws them on a map of the bed; otherwise it
lists them by name.

**Matching colours (just for fun).** The screenshots show a benchy in Sailhand's accent colour: set the filament
colour in the slicer to `#FD7E14` (RGB 253, 126, 20) and the thumbnail matches the screen.

## Macros

Sailhand calls macros by name with their documented parameters and makes no assumptions about what they do inside.
Where a macro is missing, its button does not appear.

### What Sailhand looks for

No macro is required. Pause, resume and cancel go through Moonraker's print API, which runs Klipper's `PAUSE`,
`RESUME` and `CANCEL_PRINT` (from `[pause_resume]`, or the macros that replace them, as mainsail-config does).

Macros Sailhand recognises by name and uses when they exist:

| Macro                                        | Comes from                                                                     | Used for                                                                                          |
| -------------------------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------- |
| `LOAD_FILAMENT`, `UNLOAD_FILAMENT`           | a naming convention documented by [KlipperScreen][ks-macros]; Mainsail and Fluidd call the same names | the Load and Unload buttons without Happy Hare, called without parameters ([below](#load-and-unload-filament)) |
| `SET_PAUSE_NEXT_LAYER`, `SET_PAUSE_AT_LAYER` | [mainsail-config][mainsail-config] `client.cfg`, with `ENABLE` since v1.1.0    | planning a pause at a layer, with `ENABLE` and `LAYER` ([below](#pause-at-a-layer))               |
| `T0` … `Tn` (any `T<n>` command)             | your macros, or a plugin that registers them (Happy Hare, tool changers)       | the comparison before a print: a tool of the file without its `T<n>` command is marked, because the print would stop there |

Every other macro shows in the macro list (Routines › Macros), except those whose name starts with `_` and those that replace
a built-in command (`rename_existing`). Their parameters are read from the template (`params.NAME`, with
`|default(...)` as the placeholder), as Mainsail and KlipperScreen do; none is treated as required.

Variables Sailhand reads from macros, only where the variable is declared under exactly this name. A missing
variable leaves the macro a black box:

| Macro                  | Variables                            | Comes from                                                                  | Used for                                                                     |
| ---------------------- | ------------------------------------ | --------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `SET_PRINT_STATS_INFO` | `pause_next_layer`, `pause_at_layer` | [mainsail-config][mainsail-config] `client.cfg`, since v1.1.0               | the planned layer pause, also one planned in another client ([below](#pause-at-a-layer)) |
| `T0` … `Tn`            | `spool_id`                           | your tool macros; the convention of Fluidd (since v1.28.0) and Mainsail (since v2.13.0) | the Spoolman spool of each tool head; Sailhand also assigns it ([below](#spools-of-tool-heads)) |

[ks-macros]: https://github.com/KlipperScreen/KlipperScreen/blob/master/docs/macros.md
[mainsail-config]: https://github.com/mainsail-crew/mainsail-config

### Load and unload filament

Without Happy Hare the **Load** and **Unload** buttons of the Filament page run the macros `LOAD_FILAMENT` and
`UNLOAD_FILAMENT`, when the printer has them. Sailhand calls them without parameters, so they need defaults for
everything they take. KlipperScreen's documentation has
[example macros](https://klipperscreen.readthedocs.io/en/latest/macros/) for both. With Happy Hare the buttons load
and unload through the MMU instead.

### Pause at a layer

A tap on the layer tile of the print page plans a pause. One counter starts at the next layer and shows below each
layer the height it will have (an estimate with a `~`: the current height plus the slicer's layer height per layer,
so a variable layer height makes it wrong). Sailhand pauses at the next layer with `SET_PAUSE_NEXT_LAYER` and at a
later one with `SET_PAUSE_AT_LAYER`, both from Mainsail's
[`client.cfg`](https://github.com/mainsail-crew/mainsail-config) (mainsail-config), which most Mainsail installations
include already. The tile offers the pause while a print runs, at least one of the macros exists and the slicer
reports the layers (see [Layers](#slicer-settings) above). The dialog also takes a planned pause back.

The plan itself lives in the variables `pause_next_layer` and `pause_at_layer` of mainsail-config's
`SET_PRINT_STATS_INFO`. Sailhand reads them, so a pause planned here or in another client shows on the layer tile
(the layer, or `next`, below the pause icon) and in the dialog. With a different `SET_PRINT_STATS_INFO` that lacks
these variables, the plan is not shown and the dialog always offers to take it back.

### Spools of tool heads

Moonraker knows one active spool. A tool changer has a spool on each head, and Fluidd and Mainsail keep it in a
variable `spool_id` of the tool's macro. Sailhand follows the same convention: when `gcode_macro T<n>` declares
`spool_id`, its value is the spool of that head, and choosing a spool for the head on the display sends what Fluidd
and Mainsail send:

```
SET_GCODE_VARIABLE MACRO=T0 VARIABLE=spool_id VALUE=12
SAVE_VARIABLE VARIABLE=t0__spool_id VALUE=12
```

The macro name is used as written in the configuration (Klipper matches it exactly), the saved variable in lower
case, as Fluidd (since v1.32.0) and Mainsail (since v2.14.0) save it; their earlier versions saved it in upper
case. The second line only when the printer has `[save_variables]`; restoring the value at start-up is up to the
macros. Without the variable a head has no spool of its own, and the head in use shows Moonraker's active spool. A tool
changer plugin that registers `T<n>` itself skips a name that a `gcode_macro` already has, so the variable needs a
macro `T<n>` of your own that calls the plugin's tool change.


A head's macro can also declare `variable_active`, the convention Fluidd uses for "this head's spool is the active
one". When it is true, choosing a spool for that head also sets Moonraker's active spool. Moonraker books the extruded
filament on that one active spool, so the tool change macro has to set it whenever the head changes; Klipper
can tell Moonraker with the `SET_ACTIVE_SPOOL` macro from the
[Moonraker documentation](https://moonraker.readthedocs.io/en/latest/configuration/#setting-the-active-spool-from-klipper):

```ini
[gcode_macro SET_ACTIVE_SPOOL]
gcode:
  {% if params.ID %}
    {% set id = params.ID|int %}
    {action_call_remote_method(
       "spoolman_set_active_spool",
       spool_id=id
    )}
  {% else %}
    {action_respond_info("Parameter 'ID' is required")}
  {% endif %}

[gcode_macro CLEAR_ACTIVE_SPOOL]
gcode:
  {action_call_remote_method(
    "spoolman_set_active_spool",
    spool_id=None
  )}
```

Your own `T<n>` macro (or the tool change macro after the head has changed) then calls
`SET_ACTIVE_SPOOL ID={printer["gcode_macro T<n>"].spool_id}`, with the head's number in place of `<n>`.

## Spoolman

Two questions are separate. **Booking:** Moonraker books the extrusion of the active extruder on one active spool.
With Spoolman configured this works in every setup; what differs is who sets the active spool. **Filament data
without Spoolman** (material, colour, temperature) exists only where a system stores it: the gate map of Happy Hare
and the lane of AFC. A tool changer, and a printer without a changer, has none.

Sailhand shows and compares the copy the printer works with (the Happy Hare gate map or the AFC lane) and adds what
only Spoolman knows: the weight left and all colours of a multi-colour spool. It never overwrites that copy with
Spoolman values by itself. If spool and copy differ, the gate or lane dialog shows both and offers
**Take from Spoolman**. Testing is limited to a few printers: Spoolman with Happy Hare in `push` and `readonly` runs on
a real printer; the other modes, AFC and several heads were checked against a simulated printer built from the
sources of these projects.

### Happy Hare

Happy Hare's `spoolman_support` in `[mmu_parameters]` decides who leads:

| Mode | Spool at the gate | Command when assigning | Material, colour, temperature at the gate | Take from Spoolman |
| --- | --- | --- | --- | --- |
| `off` | none; pick the spool in use at the extruder (Filament › Extruder, print page) | – | editable | – |
| `readonly` | yes | `MMU_GATE_MAP GATE= SPOOLID=` | editable without a spool, read-only with one | `MMU_SPOOLMAN REFRESH=1` |
| `push` | yes | `MMU_GATE_MAP GATE= SPOOLID=` | editable without a spool, read-only with one | `MMU_SPOOLMAN REFRESH=1` |
| `pull` | yes | `MMU_SPOOLMAN GATE= SPOOLID=` (to empty: `MMU_SPOOLMAN GATE=`) | read-only; Happy Hare takes them from Spoolman | `MMU_SPOOLMAN REFRESH=1` |

- **`push` is the recommended mode.** Happy Hare keeps the assignment and writes it to Spoolman.
- `pull`: Spoolman leads; the gate of a spool is set in Spoolman, and Sailhand does it with `MMU_SPOOLMAN`.
- `readonly`: see [Troubleshooting](troubleshooting.md#colours-or-materials-from-spoolman-are-missing-at-the-gates);
  use it once Happy Hare issue [#1571](https://github.com/moggieuk/Happy-Hare/issues/1571) is fixed, or send
  `MMU_SPOOLMAN REFRESH=1` after every Moonraker start.
- `off`: Happy Hare without Spoolman. The gates carry no spool; the active spool is chosen at the extruder, as on a
  printer without an MMU.

**Take from Spoolman** sends `MMU_SPOOLMAN REFRESH=1`. Happy Hare reads all spools again and every gate gets the
values of its spool, so do not use it during a print (Happy Hare's documentation says the same).
`MMU_GATE_MAP GATE= SPOOLID=` with the ID that is already set fetches nothing.

Sailhand compares material, first colour and, for Happy Hare, the nozzle temperature. Happy Hare uses the larger of
`settings_extruder_temp` and `default_extruder_temp`; Sailhand compares with the same rule. Happy Hare stores one
colour per gate; if the first colour of the spool matches it, the tile shows all colours of the spool.

Happy Hare (except in `off`) sets the active spool itself after loading and clears it when unloading starts. If the
loaded gate gets another spool, Happy Hare changes the active spool at the next load.

### AFC

A spool is assigned with `SET_SPOOL_ID`. With a spool, material, colour and temperature are read-only: AFC
overwrites them at the next `SET_SPOOL_ID` and from Spoolman at every restart. **Take from Spoolman** sends
`SET_SPOOL_ID LANE=<lane> SPOOL_ID=<same id>`, after which AFC fetches the spool again. AFC sets the active spool when
a lane is loaded into a tool.

Without Spoolman Sailhand shows AFC's own weight left, marked with `~`: AFC subtracts it while printing. A lane
without spool data starts at 1000 g when loaded, so the `~` is AFC's estimate.

### Tool changers

The plugins ([viesturz's klipper-toolchanger](https://github.com/viesturz/klipper-toolchanger), KTC-easy) have no
Spoolman code of their own. The spool of a head is kept as described in
[Spools of tool heads](#spools-of-tool-heads), including `variable_active` and the `SET_ACTIVE_SPOOL` macro that
makes Moonraker book on the right spool after a head change.

### The active spool

With Spoolman connected, a loaded filament and the printer at rest, Sailhand compares the spool of the lane in use
(Happy Hare: the loaded gate; AFC: the loaded lane; tool changer: the mounted head) with Moonraker's active spool.
If they differ, the tile shows a warning, and the gate or lane dialog (for a head: below the spool strip) shows a line
"Active spool" with the button **Set as active spool**. A printer without a changer, and Happy Hare in `off`, has no
such comparison: the active spool is its spool.

### Limits of Moonraker

- Moonraker has exactly one active spool. Consumption per head at the same time needs an extension of Moonraker
  (see [moonraker#1057](https://github.com/Arksine/moonraker/issues/1057), pull request #1059, both open).
- Moonraker does not tell clients when a spool changes; Sailhand reads the spools of the lanes again every 60 seconds.
- `color` of a head's macro `T<n>` is not read by Sailhand, and a Spoolman that is offline has no special handling.

### Happy Hare next to a tool changer

Happy Hare's `update_t_macros` writes `spool_id` and `color` to `gcode_macro T<n>` for the tools 0 up to the number
of gates minus one. It removes `spool_id` when the gate is empty, has no spool or `t_macro_color` is `off`. It does
not check whether such a macro is a head of the changer. If heads and Happy Hare tools share the names `T<n>`, the
spool of a head can be overwritten by this.
