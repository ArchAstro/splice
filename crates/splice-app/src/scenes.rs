//! Scenes are React components Clapper renders (github.com/ArchAstro/clapper).
//! Each scene gets a small Clapper project in the cache while it is edited:
//! Splice writes the canvas and length into `src/index.tsx`, the scene's code
//! into `src/scene.tsx`, and renders to ProRes 4444 so what the scene leaves
//! empty stays see-through on the timeline.

use crate::session::dirs;
use anyhow::{Context, Result};
use splice_core::model::{Scene, Settings};
use splice_core::time::{SECOND, Time};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const VERSION: &str = "0.4.0";
const LAUNCHER_URL: &str = "https://github.com/ArchAstro/clapper/releases/download/v0.4.0/clapper-darwin-arm64";
const LAUNCHER_SHA256: &str = "d7d9f63dd4bcac003492aa54b5b35fe04f87041af97e83a84340b4ce60e6c1cf";

/// Renders with this Clapper name the same bytes.
pub const ENGINE: &str = "splice-scene-1|clapper-0.4.0";

/// The file a scene's code lives in, inside its project.
pub const SOURCE: &str = "src/scene.tsx";

/// What a new scene starts as: a little diagram that builds itself.
pub const STARTER: &str = r##"import { AbsoluteFill, Easing, interpolate, spring, useFrame, useVideoConfig } from "@archastro/clapper-core";

// Everything on screen is a function of the frame. Clapper renders it frame by
// frame; what you leave empty stays see-through over the timeline.
const steps = ["Idea", "Draft", "Ship"];

export default function Scene() {
  const frame = useFrame();
  const { fps, width, height } = useVideoConfig();
  const gap = width / (steps.length + 1);
  return (
    <AbsoluteFill style={{ fontFamily: "Avenir Next, Helvetica, sans-serif" }}>
      <svg width={width} height={height} style={{ position: "absolute", inset: 0 }}>
        {steps.slice(1).map((_, i) => {
          const p = interpolate(frame, [20 + i * 14, 40 + i * 14], [0, 1], {
            extrapolate: "clamp",
            easing: Easing.outCubic,
          });
          const x0 = gap * (i + 1) + 110;
          const x1 = gap * (i + 2) - 110;
          return (
            <line key={i} x1={x0} x2={x0 + (x1 - x0) * p} y1={height / 2} y2={height / 2}
              stroke="#f3eee4" strokeWidth={6} strokeLinecap="round" />
          );
        })}
      </svg>
      {steps.map((label, i) => {
        const s = spring({ frame: frame - i * 14, fps, config: { damping: 13 } });
        return (
          <div key={label} style={{
            position: "absolute", left: gap * (i + 1) - 100, top: height / 2 - 60,
            width: 200, height: 120, borderRadius: 20, background: "#ef6a55",
            color: "#fff", fontSize: 38, fontWeight: 700,
            display: "flex", alignItems: "center", justifyContent: "center",
            transform: `scale(${s})`, boxShadow: "0 12px 32px rgba(0,0,0,.35)",
          }}>
            {label}
          </div>
        );
      })}
    </AbsoluteFill>
  );
}
"##;

/// The Clapper launcher: `$SPLICE_CLAPPER`, `clapper` on the PATH, or the one
/// Splice fetched.
pub fn launcher() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SPLICE_CLAPPER").map(PathBuf::from) {
        return path.exists().then_some(path);
    }
    if let Some(path) = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join("clapper"))
        .find(|p| p.is_file())
    {
        return Some(path);
    }
    let ours = managed_launcher();
    ours.exists().then_some(ours)
}

fn managed_launcher() -> PathBuf {
    dirs().data.join("bin").join(format!("clapper-{VERSION}"))
}

/// Finds Clapper, fetching its launcher (and, on first use, its runtime) when
/// it is not installed. Blocking; the first run downloads a few hundred MB.
pub fn ensure_launcher() -> Result<PathBuf> {
    if let Some(path) = launcher() {
        return Ok(path);
    }
    let path = managed_launcher();
    let dir = path.parent().expect("a folder");
    std::fs::create_dir_all(dir)?;
    let partial = dir.join("clapper.part");
    let status = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--output"])
        .arg(&partial)
        .arg(LAUNCHER_URL)
        .status()
        .context("running curl")?;
    if !status.success() {
        anyhow::bail!("could not download Clapper (curl {status})");
    }
    let bytes = std::fs::read(&partial)?;
    use sha2::Digest;
    let hex: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hex != LAUNCHER_SHA256 {
        let _ = std::fs::remove_file(&partial);
        anyhow::bail!("the Clapper download did not match its checksum");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&partial, &path)?;
    // The runtime (Node, Chromium, ffmpeg) comes down on first use.
    run(Command::new(&path).args(["runtime", "path"]))?;
    Ok(path)
}

fn run(command: &mut Command) -> Result<String> {
    let output = command.stdin(Stdio::null()).output().context("running Clapper")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!("{}", tidy_error(&format!("{err}\n{out}")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The useful lines of a Clapper failure: the error, not the stack.
pub fn tidy_error(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !l.trim_start().starts_with("at ") && !l.starts_with("Frames ") && !l.starts_with("Bundling"))
        .collect();
    let picked: Vec<&str> = lines
        .iter()
        .position(|l| l.contains("rror"))
        .map_or(&lines[lines.len().saturating_sub(6)..], |i| {
            &lines[i..(i + 8).min(lines.len())]
        })
        .to_vec();
    picked.join("\n")
}

/// The folder a scene's Clapper project lives in.
pub fn workspace(scene: &Scene) -> PathBuf {
    dirs().cache.join("scenes").join(&scene.workspace)
}

/// A fresh name for a new scene's folder.
pub fn new_workspace() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    blake3::hash(format!("{nanos}-{}", std::process::id()).as_bytes()).to_hex()[..16].to_string()
}

fn frames(duration: Time, fps: u32) -> u64 {
    ((duration as f64 / SECOND as f64) * fps as f64).round().max(1.0) as u64
}

fn entry(settings: &Settings, scene: &Scene) -> String {
    format!(
        r#"// Written by Splice: the canvas and length of this scene. Edit scene.tsx.
import {{ Composition, registerRoot }} from "@archastro/clapper-core";
import Scene from "./scene";

function Root() {{
  return <Composition id="scene" component={{Scene}} width={{{w}}} height={{{h}}} fps={{{fps}}} durationInFrames={{{n}}} />;
}}
registerRoot(Root);
"#,
        w = settings.width,
        h = settings.height,
        fps = settings.fps,
        n = frames(scene.duration, settings.fps),
    )
}

/// Makes the scene's Clapper project if it is missing, and brings its canvas,
/// length and code up to date. Writes only what changed, so a running studio
/// does not reload for nothing.
pub fn prepare(launcher: &Path, settings: &Settings, scene: &Scene) -> Result<PathBuf> {
    let dir = workspace(scene);
    if !dir.join("clapper.json").exists() {
        let parent = dir.parent().expect("a folder");
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::remove_dir_all(&dir);
        run(Command::new(launcher).arg("new").arg(&dir).current_dir(parent))?;
        for stray in ["src/data.ts", "src/theme.css"] {
            let _ = std::fs::remove_file(dir.join(stray));
        }
        std::fs::write(
            dir.join("README.md"),
            "A Splice scene. Edit src/scene.tsx; Splice picks up every save and re-renders.\n\
             `clapper preview` opens the Clapper studio on it.\n",
        )?;
    }
    write_if_changed(&dir.join("src/index.tsx"), &entry(settings, scene))?;
    write_if_changed(&dir.join(SOURCE), &scene.source)?;
    let config = serde_json::json!({ "runtime": VERSION, "entry": "src/index.tsx", "composition": "scene" });
    write_if_changed(&dir.join("clapper.json"), &format!("{config:#}\n"))?;
    Ok(dir)
}

fn write_if_changed(path: &Path, text: &str) -> Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text)?;
    }
    Ok(())
}

/// The scene's code as it is on disk now, and when it last changed.
pub fn read_source(scene: &Scene) -> Option<(String, std::time::SystemTime)> {
    let path = workspace(scene).join(SOURCE);
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    Some((std::fs::read_to_string(&path).ok()?, modified))
}

/// Renders the scene to `out` (a .mov), calling `progress` with 0–1 as frames
/// land. Blocking.
pub fn render(settings: &Settings, scene: &Scene, out: &Path, mut progress: impl FnMut(f32)) -> Result<()> {
    let launcher = ensure_launcher()?;
    let dir = prepare(&launcher, settings, scene)?;
    let target = dir.join("out").join("splice.mov");
    let _ = std::fs::remove_file(&target);
    let mut command = Command::new(&launcher);
    command
        .args(["render", "-c", "scene", "--loudnorm", "off", "-o"])
        .arg(&target)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if scene.transparent {
        command.arg("--transparent");
    }
    let mut child = command.spawn().context("starting Clapper")?;
    let mut stderr = child.stderr.take().expect("piped");
    let mut log = String::new();
    let mut buf = [0u8; 4096];
    let mut line = String::new();
    loop {
        let n = stderr.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buf[..n]);
        log.push_str(&text);
        for c in text.chars() {
            if c == '\r' || c == '\n' {
                if let Some(p) = parse_progress(&line) {
                    progress(p);
                }
                line.clear();
            } else {
                line.push(c);
            }
        }
    }
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let status = child.wait()?;
    if !status.success() || !target.exists() {
        anyhow::bail!("{}", tidy_error(&format!("{log}\n{stdout}")));
    }
    std::fs::rename(&target, out).or_else(|_| std::fs::copy(&target, out).map(|_| ()))?;
    Ok(())
}

/// `Frames 12/120 (10%)` → 0.1.
fn parse_progress(line: &str) -> Option<f32> {
    let rest = line.trim().strip_prefix("Frames ")?;
    let (done, rest) = rest.split_once('/')?;
    let total = rest.split_whitespace().next()?;
    let (done, total): (f32, f32) = (done.parse().ok()?, total.parse().ok()?);
    (total > 0.0).then(|| (done / total).clamp(0.0, 1.0))
}

/// Clapper's studio on a scene: its own preview, timeline and code panel in
/// the browser, hot-reloading as `scene.tsx` changes.
pub struct Studio {
    pub url: String,
    child: Child,
}

impl Drop for Studio {
    /// Stops the launcher and the server it started: the studio runs in its
    /// own process group.
    fn drop(&mut self) {
        let group = format!("-{}", self.child.id());
        let _ = Command::new("kill").args(["-TERM", "--", &group]).status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts the studio on a free port and returns once it answers.
pub fn studio(settings: &Settings, scene: &Scene) -> Result<Studio> {
    let launcher = ensure_launcher()?;
    let dir = prepare(&launcher, settings, scene)?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    use std::os::unix::process::CommandExt;
    let child = Command::new(&launcher)
        .process_group(0)
        .args(["preview", "--port", &port.to_string()])
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the Clapper studio")?;
    let studio = Studio {
        url: format!("http://127.0.0.1:{port}/?composition=scene"),
        child,
    };
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(30) {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(studio);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    anyhow::bail!("the Clapper studio did not start")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_progress() {
        assert_eq!(parse_progress("Frames 12/120 (10%)"), Some(0.1));
        assert_eq!(parse_progress("Bundling…"), None);
    }

    #[test]
    fn writes_the_canvas_and_length() {
        let scene = Scene {
            source: STARTER.into(),
            duration: 4 * SECOND,
            transparent: true,
            workspace: "x".into(),
        };
        let settings = Settings {
            width: 1080,
            height: 1920,
            fps: 30,
        };
        let text = entry(&settings, &scene);
        assert!(text.contains("width={1080} height={1920} fps={30} durationInFrames={120}"));
    }

    #[test]
    fn keeps_the_error_not_the_stack() {
        let log = "Bundling…\nError: Frame 3 of scene threw\n    at Scene (scene.tsx:4:2)\n    at render\nReferenceError: foo is not defined\n";
        let tidy = tidy_error(log);
        assert!(tidy.starts_with("Error: Frame 3"));
        assert!(!tidy.contains("at Scene"));
    }
}
