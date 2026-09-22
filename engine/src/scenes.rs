//! The scenes in the packs directory, and which of them the options pick.

use std::path::Path;

use crate::cli::Args;
use crate::{Result, pack, say};

fn no_packs(packs: &Path) -> String {
    format!(
        "no scene packs in {} (build them with molokolive-pack, or pass --packs)",
        packs.display()
    )
}

/// Every directory in `packs` with a manifest, sorted.
pub fn available(packs: &Path) -> Result<Vec<String>> {
    let entries = std::fs::read_dir(packs).map_err(|_| no_packs(packs))?;
    let mut scenes: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("manifest.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    scenes.sort();
    if scenes.is_empty() {
        return Err(no_packs(packs).into());
    }
    Ok(scenes)
}

/// The scenes to rotate through: --scenes (every scene by default) minus --skip-scenes, both with `*` wildcards.
pub fn select(args: &Args) -> Result<Vec<String>> {
    let available = available(&args.packs)?;
    let matching = |patterns: &[String], option: &str| -> Result<Vec<String>> {
        let mut found = Vec::new();
        for pattern in patterns {
            let matched: Vec<String> = available.iter().filter(|s| wildcard(pattern, s)).cloned().collect();
            if matched.is_empty() {
                return Err(
                    format!("{option}: no scene matches '{pattern}' (molokolive --scenes --list shows them)").into(),
                );
            }
            found.extend(matched);
        }
        Ok(found)
    };
    let mut scenes = if args.scenes.is_empty() {
        available.clone()
    } else {
        matching(&args.scenes, "--scenes")?
    };
    let skipped = matching(&args.skip_scenes, "--skip-scenes")?;
    scenes.retain(|s| !skipped.contains(s));
    scenes.sort();
    scenes.dedup();
    if scenes.is_empty() {
        return Err("--skip-scenes leaves no scenes to show".into());
    }
    if let Some(start) = &args.start_scene {
        if !available.contains(start) {
            return Err(format!("no scene '{start}' (molokolive --scenes --list shows them)").into());
        }
        if !scenes.contains(start) {
            return Err(format!("--start-scene {start} isn't in the rotation (see --scenes and --skip-scenes)").into());
        }
    }
    Ok(scenes)
}

/// Whether `name` matches `pattern`, where each `*` stands for any run of characters.
pub fn wildcard(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == name;
    };
    if name.len() < first.len() + last.len() || !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Print the scenes with what each has, for `--scenes --list`: every scene, or the ones --scenes and
/// --skip-scenes pick.
pub fn print(args: &Args) -> Result<()> {
    let everything = available(&args.packs)?;
    let filtered = !args.scenes.is_empty() || !args.skip_scenes.is_empty();
    let scenes = if filtered { select(args)? } else { everything.clone() };
    let home = std::env::var("HOME").unwrap_or_default();
    let shown = args.packs.display().to_string();
    let shown = match shown.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => shown,
    };
    if filtered {
        say!(
            "{} of the {} scenes in {shown}, as --scenes and --skip-scenes pick them:\n",
            scenes.len(),
            everything.len()
        );
    } else {
        say!("Scenes in {shown}:\n");
    }
    let width = scenes.iter().map(String::len).max().unwrap_or(0);
    for name in &scenes {
        let summary = pack::summary(&args.packs.join(name))?;
        let mut features = Vec::new();
        if summary.sky {
            features.push("sky");
        }
        if summary.reflection {
            features.push("reflection");
        }
        features.push(if summary.animated { "animated" } else { "still" });
        say!("  {name:width$}   {}", features.join(", "));
    }
    say!(
        "\nUse the names with --scenes, e.g. molokolive --scenes {},{}, or with --start-scene.",
        everything[0],
        everything.get(1).unwrap_or(&everything[0])
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::wildcard;

    #[test]
    fn wildcards_match_runs_of_characters() {
        assert!(wildcard("mini_cg_*", "mini_cg_run"));
        assert!(wildcard("*", "anything"));
        assert!(wildcard("*_far", "cg_fall_far"));
        assert!(wildcard("cg_*_close", "cg_fall_close"));
        assert!(wildcard("a*b*c", "axxbyyc"));
        assert!(wildcard("cg_floor", "cg_floor"));
        assert!(!wildcard("cg_floor", "cg_floor2"));
        assert!(!wildcard("a*b*c", "acb"));
        assert!(!wildcard("mini_*", "cg_mini_1"));
        assert!(!wildcard("ab*", "a"));
    }
}
