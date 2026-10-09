// Modified copy of alacritty/build.rs at def47a5dd511ed20607eb5d9e5e7140afd16b31d.
// Changes: removed the git VERSION env var (only used by the removed CLI) and the
// Windows resource embedding. The OpenGL binding generation is unchanged except for the
// `GL_ARB_clear_texture` extension added by ayosec's graphics patch (tag v0.16.0-graphics).

use std::env;
use std::fs::File;
use std::path::Path;

use gl_generator::{Api, Fallbacks, GlobalGenerator, Profile, Registry};

fn main() {
    let dest = env::var("OUT_DIR").unwrap();
    let mut file = File::create(Path::new(&dest).join("gl_bindings.rs")).unwrap();

    Registry::new(Api::Gl, (3, 3), Profile::Core, Fallbacks::All, [
        "GL_ARB_blend_func_extended",
        "GL_ARB_clear_texture",
        "GL_KHR_robustness",
        "GL_KHR_debug",
    ])
    .write_bindings(GlobalGenerator, &mut file)
    .unwrap();
}
