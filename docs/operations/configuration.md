# Configuration

Everything the application reads at start that is not code: the JSON files
in `runtime/config/`, the secrets they pull from `.env`, what the launcher
passes to the host, and what the modules write under `runtime/`. The host
side is `veldcore/platform/host/core/src/config.rs`; the launcher is
`buildgen/run-native.py`.

## Where the host looks

The host binary takes `--config <dir>`; the launcher passes `runtime/config`
unless told otherwise, and the binary started by hand defaults to `config`
under the working directory. The parent of the config directory is the
**runtime directory**, the base of every relative path: those in
`services.json`, and those a module asks the platform for (`resolve_path` in
`veldcore/platform/host/util/src/lib.rs` joins `runtime_dir`). So the tree
under `runtime/` is one: `config/`, `data/`, `state/`, `logs/`, and `assets/`
— the fonts `ui-service` compiles in at build (`include_bytes!` in
`veldmodules/ui-service/src/state.rs`); nothing reads `assets/` at run time.

## Files

| File | Read by | Keys |
|---|---|---|
| `services.json` | host (`ServicesManifest`) | `plugins_dir` — where the wasm modules are, relative to the runtime directory, `../build/plugins` when absent; `logs` — the log file, relative to the runtime directory, `logs/host.log` when absent; `trace.log` is written beside it |
| `core.json` | host (`CoreConfig` in `veldcore/platform/host/core/src/lib.rs`) | `log_filter` — what the console and `host.log` show; `trace_filter` — what `trace.log` keeps; `log_rate_limit_ms` — the least interval between two identical human-readable lines (`0` — none), which does not touch `trace.log`; each key has a default in code, and a missing file means the defaults |
| `<name>.json` | the module named `<name>` | whatever its `Config` type declares (below) |

The name of a module's file is the name the module reports through
`get_service_name`; `services.json` lists no modules — the host loads every
`.wasm` in `plugins_dir` and asks each for its name
(`veldcore/platform/host/core/src/plugins.rs`). The file goes to the module's
`init` whole, as one JSON, where its `Config` type parses it with serde
defaults; a module whose file is missing gets `{}` and a warning in the log —
usually a schema renamed without its config. Every `.json` in the directory
other than `services` and `core` is read before any module loads, because one
of them declares the window.

`plugins_dir` names the same directory as `plugins_dir` in `workspace.yaml`,
from a different base (the project root); the build checks that the two agree
and stops when they do not (`_check_plugins_dir_consistency` in
`buildgen/build.py`).

## Module keys

| Module | Key | Meaning |
|---|---|---|
| `data-browser` | `window` | the window this module owns: `title`, `width` and `height` in logical pixels, `ui_scale` — the floor of the interface scale (the host sends modules the greater of the window's scale factor and this, because winit on X11 and WSLg often reports `1.0` on HiDPI screens), `resizable`, `fullscreen`, `position` (`x`, `y`; absent — centred); the defaults are in `PluginWindowConfig` (`veldcore/platform/host/runners/desktop/src/window.rs`). Exactly one module declares a window — [limitations](../limitations.md) |
| `data-browser` | `initial_view` | the tab the window opens on when there is no saved layout: `search` (also when absent) or `browse` |
| `data-provider` | `access_key`, `secret_key` | the S3 credentials of Copernicus Data Space; without them the catalogue listing, remote viewing and downloads answer with the advice `NO_KEYS` (`veldmodules/data-provider/src/cdse.rs`); searching works, the catalogue's metadata are public |
| `tile-cache` | `cache_limit_mb` | the disk cache of tiles; the default is `default_cache_limit_mb` in `veldmodules/tile-cache/src/module.rs` |
| `globe`, `image-view` | `vram_budget_mb` | the video memory for tiles; the default is `DEFAULT_VRAM_BUDGET_MB` of the tiler's wrap; eviction forgets a tile, which stays on disk |

`data-library`, `image-tiler` and `ui-service` take no keys; their files hold
`{}`.

## Secrets: `${VAR}` and `.env`

Every config file is read as text, and `${VAR}` (letters, digits, underscore)
is replaced with the environment variable before parsing (`expand_env_vars`);
a variable that is not set becomes an empty string and a warning on stderr —
the logger is not up yet. Before reading any config the host loads `.env`: the
first found of `.env` in the working directory and `.env` two levels above the
config directory, that is the project root (`main` in
`veldcore/platform/host/runners/desktop/src/main.rs`, then `load_dotenv`).
Lines are `KEY=VALUE`, `#` starts a comment, paired quotes are stripped, and a
variable already set in the environment is not overridden. The host reads
`.env` itself, not the launcher, so `${VAR}` expands however the binary is
started.

The configs are in git and `.env` is not (`.gitignore`), so a secret goes only
into `.env`: `data-provider.json` holds `${COPERNICUS_ACCESS_KEY}` and
`${COPERNICUS_ACCESS_SECRET}`, `.env.example` names them and says where the
keys are issued, and `buildgen/tests/test_provider_keys.py` holds the three
places — the config, the example and the advice text — to the same names.

## The launcher

`python3 buildgen/run-native.py` starts the built host binary from
`veldcore/target/<profile>/` and does not build it: a missing binary is a
message to run the build. Its flags: `--debug` runs the debug build (the
release one otherwise; a debug build is not what is checked — [build](build.md));
`--config <dir>` names the config directory; everything else is handed to the
host as it is. The launcher silences the GPU driver's own chatter
(`EGL_LOG_LEVEL`, `MESA_DEBUG`), does not set `RUST_LOG`, hands a `SIGTERM`
it receives over to the host, and exits with the host's code — the scenario
runner reads its verdict from it.

## Environment variables

| Variable | Read by | Effect |
|---|---|---|
| `RUST_LOG` | host logger | replaces `log_filter` for the console and `host.log`; `trace_filter` stays — [diagnostics](diagnostics.md) |
| `VELDMAP_SCRIPT` | desktop runner | the scenario file of a scenario run; without it none of that is active — [scenario runs](ui-tests.md) |
| `COPERNICUS_ACCESS_KEY`, `COPERNICUS_ACCESS_SECRET` | `${VAR}` in `data-provider.json` | the S3 credentials |
| wgpu's own (`WGPU_VALIDATION` and the rest of `InstanceFlags::with_env`) | desktop runner | instance flags of wgpu; the backend is not among them — the runner builds a Vulkan instance regardless |

## What the modules write

Relative to the runtime directory and ignored by git: downloads under
`DATA_DIR` (`veldmodules/data-library/src/storage.rs`), tiles under `ROOT` of
`veldmodules/tile-cache/src/layout.rs`, the window layout at `PATH` of
`handlers::persist` in `data-browser` (`runtime/state/data-browser.json`), the
logs at `logs` of `services.json`. Which of them may be deleted and what that
costs is in [diagnostics](diagnostics.md).
