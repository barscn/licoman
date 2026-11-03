# Troubleshooting

## Vulkan / GPU rendering failures

Licoman renders through GPUI's Vulkan backend. It does **not** fall back
to a software rasterizer, and it never installs drivers itself.

**"The GPU rendering prerequisites are not met" at startup**

Licoman checks two things before starting the GUI:

1. **The Vulkan loader** (`libvulkan.so.1`). Install it with your
   distribution's package:

   | Distribution | Package |
   |---|---|
   | Arch | `vulkan-icd-loader` |
   | Debian / Ubuntu | `libvulkan1` |
   | Fedora | `vulkan-loader` |

2. **At least one driver manifest (ICD JSON)** under
   `/usr/share/vulkan/icd.d/` or `/etc/vulkan/icd.d/`:

   | GPU | Arch | Debian/Ubuntu | Fedora |
   |---|---|---|---|
   | AMD / Intel (Mesa) | `mesa-vulkan-radeon` / `vulkan-intel` | `mesa-vulkan-drivers` | `mesa-vulkan-drivers` |
   | NVIDIA | vendor driver (`nvidia-utils` and similar) | `nvidia-driver` packages | akmod-nvidia |

   Verify after installing: `ls /usr/share/vulkan/icd.d/` should list at
   least one `.json` file, and `vulkaninfo --summary` (package
   `vulkan-tools` / `vulkan-validationlayers`) should show your GPU.

**Renderer failure after startup began** (a panic mentioning vulkan/blade/
adapter): the loader and driver are present but the device or surface
could not be created. Common causes, in observed order:

- A mismatched or stale user-space driver (update both the driver and the
  loader from the same distribution source).
- Running inside a nested or bridged session where the compositor does not
  export Vulkan swapchain support.
- A GPU with an out-of-tree driver stack that does not implement the
  Vulkan versions GPUI requests.

Licoman prints a pointer to this section next to any graphics-shaped
panic. If you hit one, please report the panic text plus the output of
`vulkaninfo --summary` — the crash itself is never swallowed and no state
is modified by a failed render startup.

`LICOMAN_SKIP_VULKAN_CHECK=1` bypasses the pre-start filesystem probe
(the documented escape hatch for headless CI builds of the binary); it
does not change what the renderer needs.

## No graphical session detected

Licoman must be started from inside a desktop session:
`WAYLAND_DISPLAY` (Wayland) or `DISPLAY` (X11) must be set. Launching from
a plain TTY or `ssh` without session forwarding prints the readable
diagnostic and exits; nothing is modified.

## Backend services absent

power-profiles-daemon, xfconfd and similar services are probed
**passively**: Licoman reads the bus name list and reports a truthful
"service not running" state. It never starts, installs or activates a
service. Start the service yourself (`systemctl --user start …`) if you
want those rows to go live; they will disappear again (honestly) when the
service stops.

## Preferences look wrong / theme reset

Licoman writes only its own
`$XDG_CONFIG_HOME/licoman/preferences.toml`. A corrupt file falls back to
the Dark default with a diagnostic in the Preferences window; delete the
file to regenerate it. No other file is ever touched without the
explicit write pipeline (conflict check, backup, verification — see
`docs/configuration-safety.md`).
