// Copyright 2016 Mozilla Foundation
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::compiler::args::*;
use crate::compiler::c::{CCompilerImpl, CCompilerKind, ParsedArguments};
use crate::compiler::gcc::ArgData::*;
use crate::compiler::{CCompileCommand, Cacheable, CompileCommand, CompilerArguments, gcc};
use crate::mock_command::CommandCreatorSync;
use crate::{counted_array, dist};
use async_trait::async_trait;
use std::ffi::OsString;
use std::path::Path;
use std::process;

use crate::errors::*;

/// A struct on which to implement `CCompilerImpl` for Emscripten.
#[derive(Clone, Debug)]
pub struct Emscripten {
    /// true iff this is em++ (as opposed to emcc).
    pub emcplusplus: bool,
    /// Compiler version string.
    pub version: Option<String>,
}

#[async_trait]
impl CCompilerImpl for Emscripten {
    fn kind(&self) -> CCompilerKind {
        CCompilerKind::Emscripten
    }
    fn plusplus(&self) -> bool {
        self.emcplusplus
    }
    fn version(&self) -> Option<String> {
        self.version.clone()
    }
    fn parse_arguments(
        &self,
        arguments: &[OsString],
        cwd: &Path,
        _env_vars: &[(OsString, OsString)],
    ) -> CompilerArguments<ParsedArguments> {
        gcc::parse_arguments(
            arguments,
            cwd,
            (&gcc::ARGS[..], &ARGS[..]),
            self.emcplusplus,
            self.kind(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn preprocess<T>(
        &self,
        creator: &T,
        executable: &Path,
        parsed_args: &ParsedArguments,
        cwd: &Path,
        env_vars: &[(OsString, OsString)],
        may_dist: bool,
        rewrite_includes_only: bool,
        preprocessor_cache_mode: bool,
    ) -> Result<process::Output>
    where
        T: CommandCreatorSync,
    {
        let ignorable_whitespace_flags = if preprocessor_cache_mode {
            vec![]
        } else {
            vec!["-P".to_string()]
        };
        gcc::preprocess(
            creator,
            executable,
            parsed_args,
            cwd,
            env_vars,
            may_dist,
            self.kind(),
            rewrite_includes_only,
            ignorable_whitespace_flags,
            gcc::language_to_gcc_arg,
        )
        .await
    }

    fn generate_compile_commands<T>(
        &self,
        path_transformer: &mut dist::PathTransformer,
        executable: &Path,
        parsed_args: &ParsedArguments,
        cwd: &Path,
        env_vars: &[(OsString, OsString)],
        rewrite_includes_only: bool,
    ) -> Result<(
        Box<dyn CompileCommand<T>>,
        Option<dist::CompileCommand>,
        Cacheable,
    )>
    where
        T: CommandCreatorSync,
    {
        gcc::generate_compile_commands(
            path_transformer,
            executable,
            parsed_args,
            cwd,
            env_vars,
            self.kind(),
            rewrite_includes_only,
            gcc::language_to_gcc_arg,
        )
        .map(|(command, dist_command, cacheable)| {
            (CCompileCommand::new(command), dist_command, cacheable)
        })
    }
}

// Emscripten-specific argument definitions.
// The `-s` flag is critical: it specifies settings that affect both compilation
// and linking (e.g. `-s WASM=1`, `-s USE_SDL=2`). These must be included in the
// cache key because they alter codegen output.
// Emscripten-specific argument definitions, sorted alphabetically as required
// by the argument parser.
counted_array!(pub static ARGS: [ArgInfo<gcc::ArgData>; _] = [
    // Linking-only flags — mark as too hard to cache if present.
    take_arg!("--embed-file", OsString, Separated, TooHard),
    flag!("--emrun", PassThroughFlag),
    take_arg!("--js-library", OsString, Separated, TooHard),
    take_arg!("--post-js", OsString, Separated, TooHard),
    take_arg!("--pre-js", OsString, Separated, TooHard),
    take_arg!("--preload-file", OsString, Separated, TooHard),
    // -s KEY=VALUE settings that affect compilation output.
    // Can be either `-s KEY=VALUE` (separated) or `-sKEY=VALUE` (concatenated).
    take_arg!("-s", OsString, CanBeSeparated, PassThrough),
]);

#[cfg(test)]
mod test {
    use super::*;
    use crate::compiler::c::ArtifactDescriptor;
    use crate::compiler::compiler::Language;
    use crate::compiler::*;

    fn parse_arguments_(
        arguments: Vec<String>,
        plusplus: bool,
    ) -> CompilerArguments<ParsedArguments> {
        let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        gcc::parse_arguments(
            &args,
            ".".as_ref(),
            (&gcc::ARGS[..], &ARGS[..]),
            plusplus,
            CCompilerKind::Emscripten,
        )
    }

    #[test]
    fn test_parse_arguments_simple() {
        let args = stringvec!["-c", "foo.cpp", "-o", "foo.o"];
        let ParsedArguments {
            input,
            language,
            compilation_flag,
            outputs,
            preprocessor_args,
            common_args,
            ..
        } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert_eq!(Some("foo.cpp"), input.to_str());
        assert_eq!(Language::Cxx, language);
        assert_eq!(Some("-c"), compilation_flag.to_str());
        assert_map_contains!(
            outputs,
            (
                "obj",
                ArtifactDescriptor {
                    path: "foo.o".into(),
                    optional: false
                }
            )
        );
        assert!(preprocessor_args.is_empty());
        assert!(common_args.is_empty());
    }

    #[test]
    fn test_parse_arguments_emcc_c_file() {
        let args = stringvec!["-c", "foo.c", "-o", "foo.o"];
        let ParsedArguments {
            input, language, ..
        } = match parse_arguments_(args, false) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert_eq!(Some("foo.c"), input.to_str());
        assert_eq!(Language::C, language);
    }

    #[test]
    fn test_parse_arguments_s_flag_separated() {
        let args = stringvec!["-c", "foo.cpp", "-o", "foo.o", "-s", "WASM=1"];
        let ParsedArguments { common_args, .. } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert!(common_args.contains(&OsString::from("-sWASM=1")));
    }

    #[test]
    fn test_parse_arguments_s_flag_concatenated() {
        let args = stringvec!["-c", "foo.cpp", "-o", "foo.o", "-sWASM=1"];
        let ParsedArguments { common_args, .. } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert!(common_args.contains(&OsString::from("-sWASM=1")));
    }

    #[test]
    fn test_parse_arguments_multiple_s_flags() {
        let args = stringvec![
            "-c",
            "foo.cpp",
            "-o",
            "foo.o",
            "-s",
            "WASM=1",
            "-s",
            "USE_SDL=2",
            "-s",
            "ALLOW_MEMORY_GROWTH=1"
        ];
        let ParsedArguments { common_args, .. } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert!(common_args.contains(&OsString::from("-sWASM=1")));
        assert!(common_args.contains(&OsString::from("-sUSE_SDL=2")));
        assert!(common_args.contains(&OsString::from("-sALLOW_MEMORY_GROWTH=1")));
    }

    #[test]
    fn test_parse_arguments_s_flags_affect_hash() {
        let args1 = stringvec!["-c", "foo.cpp", "-o", "foo.o", "-s", "WASM=1"];
        let args2 = stringvec!["-c", "foo.cpp", "-o", "foo.o", "-s", "WASM=0"];
        let parsed1 = match parse_arguments_(args1, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        let parsed2 = match parse_arguments_(args2, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert_ne!(parsed1.common_args, parsed2.common_args);
    }

    #[test]
    fn test_parse_arguments_no_c_flag() {
        let args = stringvec!["foo.cpp", "-o", "foo.js"];
        assert!(matches!(
            parse_arguments_(args, true),
            CompilerArguments::NotCompilation
        ));
    }

    #[test]
    fn test_parse_arguments_dash_e() {
        let args = stringvec!["-E", "foo.cpp"];
        assert!(matches!(
            parse_arguments_(args, true),
            CompilerArguments::CannotCache(_, _)
        ));
    }

    #[test]
    fn test_parse_arguments_preload_file() {
        let args = stringvec!["-c", "foo.cpp", "--preload-file", "data"];
        assert!(matches!(
            parse_arguments_(args, true),
            CompilerArguments::CannotCache(_, _)
        ));
    }

    #[test]
    fn test_parse_arguments_default_output() {
        let args = stringvec!["-c", "foo.cpp"];
        let ParsedArguments { outputs, .. } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert_map_contains!(
            outputs,
            (
                "obj",
                ArtifactDescriptor {
                    path: "foo.o".into(),
                    optional: false
                }
            )
        );
    }

    #[test]
    fn test_parse_arguments_with_clang_flags() {
        let args = stringvec!["-c", "foo.cpp", "-o", "foo.o", "-O2", "-Wall", "-std=c++17"];
        let ParsedArguments { common_args, .. } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert!(common_args.contains(&OsString::from("-O2")));
        assert!(common_args.contains(&OsString::from("-Wall")));
        assert!(common_args.contains(&OsString::from("-std=c++17")));
    }

    #[test]
    fn test_parse_arguments_with_include_paths() {
        let args = stringvec!["-c", "foo.cpp", "-o", "foo.o", "-I/usr/include", "-Ilib"];
        let ParsedArguments {
            preprocessor_args, ..
        } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert!(preprocessor_args.contains(&OsString::from("-I/usr/include")));
        assert!(preprocessor_args.contains(&OsString::from("-Ilib")));
    }

    #[test]
    fn test_parse_arguments_s_flag_with_optimization() {
        let args = stringvec!["-c", "foo.cpp", "-o", "foo.o", "-O2", "-s", "WASM=1"];
        let ParsedArguments { common_args, .. } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert!(common_args.contains(&OsString::from("-O2")));
        assert!(common_args.contains(&OsString::from("-sWASM=1")));
    }

    #[test]
    fn test_parse_arguments_embed_file() {
        let args = stringvec!["-c", "foo.cpp", "--embed-file", "data"];
        assert!(matches!(
            parse_arguments_(args, true),
            CompilerArguments::CannotCache(_, _)
        ));
    }

    #[test]
    fn test_parse_arguments_pre_js() {
        let args = stringvec!["-c", "foo.cpp", "--pre-js", "pre.js"];
        assert!(matches!(
            parse_arguments_(args, true),
            CompilerArguments::CannotCache(_, _)
        ));
    }

    #[test]
    fn test_parse_arguments_post_js() {
        let args = stringvec!["-c", "foo.cpp", "--post-js", "post.js"];
        assert!(matches!(
            parse_arguments_(args, true),
            CompilerArguments::CannotCache(_, _)
        ));
    }

    #[test]
    fn test_parse_arguments_js_library() {
        let args = stringvec!["-c", "foo.cpp", "--js-library", "lib.js"];
        assert!(matches!(
            parse_arguments_(args, true),
            CompilerArguments::CannotCache(_, _)
        ));
    }

    #[test]
    fn test_parse_arguments_emrun() {
        let args = stringvec!["-c", "foo.cpp", "-o", "foo.o", "--emrun"];
        let ParsedArguments { common_args, .. } = match parse_arguments_(args, true) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert!(common_args.contains(&OsString::from("--emrun")));
    }
}
