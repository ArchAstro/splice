//! Builds a two-layer project from an existing one: the second clip is moved
//! onto a layer above the first, overlapping it. For checking export by eye.
use splice_core::model::Project;
use splice_core::store::{Store, VersionKind};
use splice_core::time::SECOND;

fn main() {
    let path = std::env::args().nth(1).expect("project path");
    let store = Store::open(path.as_ref()).unwrap();
    let mut project: Project = store.latest().unwrap().unwrap();
    let logo = project.tracks[0].clips[1].id;
    // The 5s logo sits at 8s; lift it a layer and pull it to 2s, over the first video.
    project.move_clips(&[logo], -6 * SECOND, 1);
    project.tracks.last_mut().unwrap().muted = false;
    store
        .save_version(&project, VersionKind::Save, Some("layered"))
        .unwrap();
    for (i, t) in project.tracks.iter().enumerate() {
        for c in &t.clips {
            println!("track {i} clip {} {}..{}", c.id.0, c.start, c.end());
        }
    }
}
