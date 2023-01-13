use super::bench::SIMPLE_RAYTRACER;
use super::build_sysroot::{self, SYSROOT_SRC};
use super::config;
use super::path::{Dirs, RelPath};
use super::prepare::GitRepo;
use super::utils::{spawn_and_wait, spawn_and_wait_with_input, CargoProject, Compiler};
use super::SysrootKind;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;

static BUILD_EXAMPLE_OUT_DIR: RelPath = RelPath::BUILD.join("example");

struct TestCase {
    config: &'static str,
    func: &'static dyn Fn(&TestRunner),
}

impl TestCase {
    const fn new(config: &'static str, func: &'static dyn Fn(&TestRunner)) -> Self {
        Self { config, func }
    }
}

const NO_SYSROOT_SUITE: &[TestCase] = &[
    TestCase::new("build.mini_core", &|runner| {
        runner.run_rustc(["example/mini_core.rs", "--crate-type", "lib,dylib"]);
    }),
    TestCase::new("build.example", &|runner| {
        runner.run_rustc(["example/example.rs", "--crate-type", "lib"]);
    }),
    TestCase::new("jit.mini_core_hello_world", &|runner| {
        let mut jit_cmd = runner.rustc_command([
            "-Zunstable-options",
            "-Cllvm-args=mode=jit",
            "-Cprefer-dynamic",
            "example/mini_core_hello_world.rs",
            "--cfg",
            "jit",
        ]);
        jit_cmd.env("CG_CLIF_JIT_ARGS", "abc bcd");
        spawn_and_wait(jit_cmd);

        eprintln!("[JIT-lazy] mini_core_hello_world");
        let mut jit_cmd = runner.rustc_command([
            "-Zunstable-options",
            "-Cllvm-args=mode=jit-lazy",
            "-Cprefer-dynamic",
            "example/mini_core_hello_world.rs",
            "--cfg",
            "jit",
        ]);
        jit_cmd.env("CG_CLIF_JIT_ARGS", "abc bcd");
        spawn_and_wait(jit_cmd);
    }),
    TestCase::new("aot.mini_core_hello_world", &|runner| {
        runner.run_rustc(["example/mini_core_hello_world.rs"]);
        runner.run_out_command("mini_core_hello_world", ["abc", "bcd"]);
    }),
];

const BASE_SYSROOT_SUITE: &[TestCase] = &[
    TestCase::new("aot.arbitrary_self_types_pointers_and_wrappers", &|runner| {
        runner.run_rustc(["example/arbitrary_self_types_pointers_and_wrappers.rs"]);
        runner.run_out_command("arbitrary_self_types_pointers_and_wrappers", []);
    }),
    TestCase::new("aot.issue_91827_extern_types", &|runner| {
        runner.run_rustc(["example/issue-91827-extern-types.rs"]);
        runner.run_out_command("issue-91827-extern-types", []);
    }),
    TestCase::new("build.alloc_system", &|runner| {
        runner.run_rustc(["example/alloc_system.rs", "--crate-type", "lib"]);
    }),
    TestCase::new("aot.alloc_example", &|runner| {
        runner.run_rustc(["example/alloc_example.rs"]);
        runner.run_out_command("alloc_example", []);
    }),
    TestCase::new("jit.std_example", &|runner| {
        runner.run_rustc([
            "-Zunstable-options",
            "-Cllvm-args=mode=jit",
            "-Cprefer-dynamic",
            "example/std_example.rs",
        ]);

        eprintln!("[JIT-lazy] std_example");
        runner.run_rustc([
            "-Zunstable-options",
            "-Cllvm-args=mode=jit-lazy",
            "-Cprefer-dynamic",
            "example/std_example.rs",
        ]);
    }),
    TestCase::new("aot.std_example", &|runner| {
        runner.run_rustc(["example/std_example.rs"]);
        runner.run_out_command("std_example", ["arg"]);
    }),
    TestCase::new("aot.dst_field_align", &|runner| {
        runner.run_rustc(["example/dst-field-align.rs"]);
        runner.run_out_command("dst-field-align", []);
    }),
    TestCase::new("aot.subslice-patterns-const-eval", &|runner| {
        runner.run_rustc(["example/subslice-patterns-const-eval.rs"]);
        runner.run_out_command("subslice-patterns-const-eval", []);
    }),
    TestCase::new("aot.track-caller-attribute", &|runner| {
        runner.run_rustc(["example/track-caller-attribute.rs"]);
        runner.run_out_command("track-caller-attribute", []);
    }),
    TestCase::new("aot.float-minmax-pass", &|runner| {
        runner.run_rustc(["example/float-minmax-pass.rs"]);
        runner.run_out_command("float-minmax-pass", []);
    }),
    TestCase::new("aot.mod_bench", &|runner| {
        runner.run_rustc(["example/mod_bench.rs"]);
        runner.run_out_command("mod_bench", []);
    }),
    TestCase::new("aot.issue-72793", &|runner| {
        runner.run_rustc(["example/issue-72793.rs"]);
        runner.run_out_command("issue-72793", []);
    }),
];

pub(crate) static RAND_REPO: GitRepo =
    GitRepo::github("rust-random", "rand", "0f933f9c7176e53b2a3c7952ded484e1783f0bf1", "rand");

static RAND: CargoProject = CargoProject::new(&RAND_REPO.source_dir(), "rand");

pub(crate) static REGEX_REPO: GitRepo =
    GitRepo::github("rust-lang", "regex", "341f207c1071f7290e3f228c710817c280c8dca1", "regex");

static REGEX: CargoProject = CargoProject::new(&REGEX_REPO.source_dir(), "regex");

pub(crate) static PORTABLE_SIMD_REPO: GitRepo = GitRepo::github(
    "rust-lang",
    "portable-simd",
    "582239ac3b32007613df04d7ffa78dc30f4c5645",
    "portable-simd",
);

static PORTABLE_SIMD: CargoProject =
    CargoProject::new(&PORTABLE_SIMD_REPO.source_dir(), "portable_simd");

static LIBCORE_TESTS: CargoProject =
    CargoProject::new(&SYSROOT_SRC.join("library/core/tests"), "core_tests");

const EXTENDED_SYSROOT_SUITE: &[TestCase] = &[
    TestCase::new("test.rust-random/rand", &|runner| {
        spawn_and_wait(RAND.clean(&runner.target_compiler.cargo, &runner.dirs));

        if runner.is_native {
            eprintln!("[TEST] rust-random/rand");
            let mut test_cmd = RAND.test(&runner.target_compiler, &runner.dirs);
            test_cmd.arg("--workspace");
            spawn_and_wait(test_cmd);
        } else {
            eprintln!("[AOT] rust-random/rand");
            let mut build_cmd = RAND.build(&runner.target_compiler, &runner.dirs);
            build_cmd.arg("--workspace").arg("--tests");
            spawn_and_wait(build_cmd);
        }
    }),
    TestCase::new("test.simple-raytracer", &|runner| {
        spawn_and_wait(SIMPLE_RAYTRACER.clean(&runner.host_compiler.cargo, &runner.dirs));
        spawn_and_wait(SIMPLE_RAYTRACER.build(&runner.target_compiler, &runner.dirs));
    }),
    TestCase::new("test.libcore", &|runner| {
        spawn_and_wait(LIBCORE_TESTS.clean(&runner.host_compiler.cargo, &runner.dirs));

        if runner.is_native {
            spawn_and_wait(LIBCORE_TESTS.test(&runner.target_compiler, &runner.dirs));
        } else {
            eprintln!("Cross-Compiling: Not running tests");
            let mut build_cmd = LIBCORE_TESTS.build(&runner.target_compiler, &runner.dirs);
            build_cmd.arg("--tests");
            spawn_and_wait(build_cmd);
        }
    }),
    TestCase::new("test.regex-shootout-regex-dna", &|runner| {
        spawn_and_wait(REGEX.clean(&runner.target_compiler.cargo, &runner.dirs));

        // newer aho_corasick versions throw a deprecation warning
        let lint_rust_flags = format!("{} --cap-lints warn", runner.target_compiler.rustflags);

        let mut build_cmd = REGEX.build(&runner.target_compiler, &runner.dirs);
        build_cmd.arg("--example").arg("shootout-regex-dna");
        build_cmd.env("RUSTFLAGS", lint_rust_flags.clone());
        spawn_and_wait(build_cmd);

        if runner.is_native {
            let mut run_cmd = REGEX.run(&runner.target_compiler, &runner.dirs);
            run_cmd.arg("--example").arg("shootout-regex-dna");
            run_cmd.env("RUSTFLAGS", lint_rust_flags);

            let input = fs::read_to_string(
                REGEX.source_dir(&runner.dirs).join("examples").join("regexdna-input.txt"),
            )
            .unwrap();
            let expected_path =
                REGEX.source_dir(&runner.dirs).join("examples").join("regexdna-output.txt");
            let expected = fs::read_to_string(&expected_path).unwrap();

            let output = spawn_and_wait_with_input(run_cmd, input);
            // Make sure `[codegen mono items] start` doesn't poison the diff
            let output = output
                .lines()
                .filter(|line| !line.contains("codegen mono items"))
                .chain(Some("")) // This just adds the trailing newline
                .collect::<Vec<&str>>()
                .join("\r\n");

            let output_matches = expected.lines().eq(output.lines());
            if !output_matches {
                let res_path = REGEX.source_dir(&runner.dirs).join("res.txt");
                fs::write(&res_path, &output).unwrap();

                if cfg!(windows) {
                    println!("Output files don't match!");
                    println!("Expected Output:\n{}", expected);
                    println!("Actual Output:\n{}", output);
                } else {
                    let mut diff = Command::new("diff");
                    diff.arg("-u");
                    diff.arg(res_path);
                    diff.arg(expected_path);
                    spawn_and_wait(diff);
                }

                std::process::exit(1);
            }
        }
    }),
    TestCase::new("test.regex", &|runner| {
        spawn_and_wait(REGEX.clean(&runner.host_compiler.cargo, &runner.dirs));

        // newer aho_corasick versions throw a deprecation warning
        let lint_rust_flags = format!("{} --cap-lints warn", runner.target_compiler.rustflags);

        if runner.is_native {
            let mut run_cmd = REGEX.test(&runner.target_compiler, &runner.dirs);
            run_cmd.args([
                "--tests",
                "--",
                "--exclude-should-panic",
                "--test-threads",
                "1",
                "-Zunstable-options",
                "-q",
            ]);
            run_cmd.env("RUSTFLAGS", lint_rust_flags);
            spawn_and_wait(run_cmd);
        } else {
            eprintln!("Cross-Compiling: Not running tests");
            let mut build_cmd = REGEX.build(&runner.target_compiler, &runner.dirs);
            build_cmd.arg("--tests");
            build_cmd.env("RUSTFLAGS", lint_rust_flags.clone());
            spawn_and_wait(build_cmd);
        }
    }),
    TestCase::new("test.portable-simd", &|runner| {
        spawn_and_wait(PORTABLE_SIMD.clean(&runner.host_compiler.cargo, &runner.dirs));

        let mut build_cmd = PORTABLE_SIMD.build(&runner.target_compiler, &runner.dirs);
        build_cmd.arg("--all-targets");
        spawn_and_wait(build_cmd);

        if runner.is_native {
            let mut test_cmd = PORTABLE_SIMD.test(&runner.target_compiler, &runner.dirs);
            test_cmd.arg("-q");
            spawn_and_wait(test_cmd);
        }
    }),
];

pub(crate) fn run_tests(
    dirs: &Dirs,
    channel: &str,
    sysroot_kind: SysrootKind,
    cg_clif_dylib: &Path,
    host_compiler: &Compiler,
    target_triple: &str,
) {
    let runner =
        TestRunner::new(dirs.clone(), host_compiler.triple.clone(), target_triple.to_string());

    if config::get_bool("testsuite.no_sysroot") {
        build_sysroot::build_sysroot(
            dirs,
            channel,
            SysrootKind::None,
            cg_clif_dylib,
            host_compiler,
            &target_triple,
        );

        BUILD_EXAMPLE_OUT_DIR.ensure_fresh(dirs);
        runner.run_testsuite(NO_SYSROOT_SUITE);
    } else {
        eprintln!("[SKIP] no_sysroot tests");
    }

    let run_base_sysroot = config::get_bool("testsuite.base_sysroot");
    let run_extended_sysroot = config::get_bool("testsuite.extended_sysroot");

    if run_base_sysroot || run_extended_sysroot {
        build_sysroot::build_sysroot(
            dirs,
            channel,
            sysroot_kind,
            cg_clif_dylib,
            host_compiler,
            &target_triple,
        );
    }

    if run_base_sysroot {
        runner.run_testsuite(BASE_SYSROOT_SUITE);
    } else {
        eprintln!("[SKIP] base_sysroot tests");
    }

    if run_extended_sysroot {
        runner.run_testsuite(EXTENDED_SYSROOT_SUITE);
    } else {
        eprintln!("[SKIP] extended_sysroot tests");
    }
}

struct TestRunner {
    is_native: bool,
    jit_supported: bool,
    dirs: Dirs,
    host_compiler: Compiler,
    target_compiler: Compiler,
}

impl TestRunner {
    pub fn new(dirs: Dirs, host_triple: String, target_triple: String) -> Self {
        let is_native = host_triple == target_triple;
        let jit_supported =
            is_native && host_triple.contains("x86_64") && !host_triple.contains("windows");

        let host_compiler = Compiler::clif_with_triple(&dirs, host_triple);

        let mut target_compiler = Compiler::clif_with_triple(&dirs, target_triple);
        if !is_native {
            target_compiler.set_cross_linker_and_runner();
        }
        if let Ok(rustflags) = env::var("RUSTFLAGS") {
            target_compiler.rustflags.push(' ');
            target_compiler.rustflags.push_str(&rustflags);
        }
        if let Ok(rustdocflags) = env::var("RUSTDOCFLAGS") {
            target_compiler.rustdocflags.push(' ');
            target_compiler.rustdocflags.push_str(&rustdocflags);
        }

        // FIXME fix `#[linkage = "extern_weak"]` without this
        if target_compiler.triple.contains("darwin") {
            target_compiler.rustflags.push_str(" -Clink-arg=-undefined -Clink-arg=dynamic_lookup");
        }

        Self { is_native, jit_supported, dirs, host_compiler, target_compiler }
    }

    pub fn run_testsuite(&self, tests: &[TestCase]) {
        for &TestCase { config, func } in tests {
            let (tag, testname) = config.split_once('.').unwrap();
            let tag = tag.to_uppercase();
            let is_jit_test = tag == "JIT";

            if !config::get_bool(config) || (is_jit_test && !self.jit_supported) {
                eprintln!("[{tag}] {testname} (skipped)");
                continue;
            } else {
                eprintln!("[{tag}] {testname}");
            }

            func(self);
        }
    }

    #[must_use]
    fn rustc_command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(&self.target_compiler.rustc);
        cmd.args(self.target_compiler.rustflags.split_whitespace());
        cmd.arg("-L");
        cmd.arg(format!("crate={}", BUILD_EXAMPLE_OUT_DIR.to_path(&self.dirs).display()));
        cmd.arg("--out-dir");
        cmd.arg(format!("{}", BUILD_EXAMPLE_OUT_DIR.to_path(&self.dirs).display()));
        cmd.arg("-Cdebuginfo=2");
        cmd.arg("--target");
        cmd.arg(&self.target_compiler.triple);
        cmd.arg("-Cpanic=abort");
        cmd.args(args);
        cmd
    }

    fn run_rustc<I, S>(&self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        spawn_and_wait(self.rustc_command(args));
    }

    fn run_out_command<'a, I>(&self, name: &str, args: I)
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut full_cmd = vec![];

        // Prepend the RUN_WRAPPER's
        if !self.target_compiler.runner.is_empty() {
            full_cmd.extend(self.target_compiler.runner.iter().cloned());
        }

        full_cmd.push(
            BUILD_EXAMPLE_OUT_DIR.to_path(&self.dirs).join(name).to_str().unwrap().to_string(),
        );

        for arg in args.into_iter() {
            full_cmd.push(arg.to_string());
        }

        let mut cmd_iter = full_cmd.into_iter();
        let first = cmd_iter.next().unwrap();

        let mut cmd = Command::new(first);
        cmd.args(cmd_iter);

        spawn_and_wait(cmd);
    }
}
