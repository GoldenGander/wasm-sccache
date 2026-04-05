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

use crate::compiler::c::{ArtifactDescriptor, CCompilerImpl, CCompilerKind, ParsedArguments};
use crate::compiler::{
    CCompileCommand, Cacheable, ColorMode, CompileCommand, CompilerArguments, Language,
    SingleCompileCommand,
};
use crate::dist;
use crate::mock_command::CommandCreatorSync;
use async_trait::async_trait;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process;

use crate::errors::*;

#[derive(Clone, Debug)]
pub struct WasmOpt {
    pub version: Option<String>,
}

#[async_trait]
impl CCompilerImpl for WasmOpt {
    fn kind(&self) -> CCompilerKind {
        CCompilerKind::WasmOpt
    }
    fn plusplus(&self) -> bool {
        false
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
        parse_arguments(arguments, cwd)
    }
    #[allow(clippy::too_many_arguments)]
    async fn preprocess<T>(
        &self,
        _creator: &T,
        _executable: &Path,
        parsed_args: &ParsedArguments,
        cwd: &Path,
        _env_vars: &[(OsString, OsString)],
        _may_dist: bool,
        _rewrite_includes_only: bool,
        _preprocessor_cache_mode: bool,
    ) -> Result<process::Output>
    where
        T: CommandCreatorSync,
    {
        let input = if parsed_args.input.is_absolute() {
            parsed_args.input.clone()
        } else {
            cwd.join(&parsed_args.input)
        };
        std::fs::read(input)
            .map_err(anyhow::Error::new)
            .map(|s| process::Output {
                status: process::ExitStatus::default(),
                stdout: s,
                stderr: vec![],
            })
    }
    fn generate_compile_commands<T>(
        &self,
        path_transformer: &mut dist::PathTransformer,
        executable: &Path,
        parsed_args: &ParsedArguments,
        cwd: &Path,
        env_vars: &[(OsString, OsString)],
        _rewrite_includes_only: bool,
    ) -> Result<(
        Box<dyn CompileCommand<T>>,
        Option<dist::CompileCommand>,
        Cacheable,
    )>
    where
        T: CommandCreatorSync,
    {
        generate_compile_commands(path_transformer, executable, parsed_args, cwd, env_vars).map(
            |(command, dist_command, cacheable)| {
                (CCompileCommand::new(command), dist_command, cacheable)
            },
        )
    }
}

pub fn parse_arguments(arguments: &[OsString], cwd: &Path) -> CompilerArguments<ParsedArguments> {
    if arguments.is_empty() {
        return CompilerArguments::NotCompilation;
    }

    let mut output = None;
    let mut input = None;
    let mut common_args = vec![];
    let mut i = 0;
    while i < arguments.len() {
        let arg = &arguments[i];
        let arg_str = arg.to_string_lossy();

        if arg_str.starts_with('@') {
            cannot_cache!("@");
        } else if arg_str == "-o" {
            let Some(next) = arguments.get(i + 1) else {
                cannot_cache!("missing argument to -o");
            };
            if next == "-" {
                cannot_cache!("-o", "-".to_string());
            }
            output = Some(cwd.join(next));
            i += 2;
            continue;
        } else if let Some(path) = arg_str.strip_prefix("--output=") {
            if path == "-" {
                cannot_cache!("-o", "-".to_string());
            }
            output = Some(cwd.join(path));
        } else if arg_str == "--output" {
            let Some(next) = arguments.get(i + 1) else {
                cannot_cache!("missing argument to --output");
            };
            if next == "-" {
                cannot_cache!("-o", "-".to_string());
            }
            output = Some(cwd.join(next));
            i += 2;
            continue;
        } else if arg == "-" {
            cannot_cache!("stdin input");
        } else if arg_str.starts_with('-') {
            common_args.push(arg.clone());
        } else {
            if input.is_some() {
                cannot_cache!("multiple input files", format!("{:?}", vec![arg]));
            }
            let detected = Language::from_file_name(Path::new(arg));
            match detected {
                Some(Language::Wasm) => input = Some(arg.clone()),
                Some(_) => cannot_cache!("unsupported input type"),
                None => cannot_cache!("unknown source language"),
            }
        }
        i += 1;
    }

    let input = match input {
        Some(input) => input,
        None => return CompilerArguments::NotCompilation,
    };
    let output = match output {
        Some(output) => output,
        None => cannot_cache!("no output file"),
    };

    let mut outputs = HashMap::new();
    outputs.insert(
        "obj",
        ArtifactDescriptor {
            path: output,
            optional: false,
        },
    );

    CompilerArguments::Ok(ParsedArguments {
        input: input.into(),
        double_dash_input: false,
        language: Language::Wasm,
        compilation_flag: OsString::new(),
        depfile: None,
        outputs,
        dependency_args: vec![],
        preprocessor_args: vec![],
        common_args,
        arch_args: vec![],
        unhashed_args: vec![],
        extra_dist_files: vec![],
        extra_hash_files: vec![],
        msvc_show_includes: false,
        profile_generate: false,
        color_mode: ColorMode::Off,
        suppress_rewrite_includes_only: false,
        too_hard_for_preprocessor_cache_mode: None,
    })
}

pub fn generate_compile_commands(
    path_transformer: &mut dist::PathTransformer,
    executable: &Path,
    parsed_args: &ParsedArguments,
    cwd: &Path,
    env_vars: &[(OsString, OsString)],
) -> Result<(
    SingleCompileCommand,
    Option<dist::CompileCommand>,
    Cacheable,
)> {
    #[cfg(not(feature = "dist-client"))]
    {
        let _ = path_transformer;
    }

    let out_file = match parsed_args.outputs.get("obj") {
        Some(obj) => &obj.path,
        None => return Err(anyhow!("Missing output file")),
    };

    let mut arguments: Vec<OsString> = vec![];
    arguments.extend_from_slice(&parsed_args.common_args);
    arguments.extend(vec![
        (&parsed_args.input).into(),
        "-o".into(),
        out_file.into(),
    ]);

    let command = SingleCompileCommand {
        executable: executable.to_owned(),
        arguments,
        env_vars: env_vars.to_owned(),
        cwd: cwd.to_owned(),
    };

    #[cfg(not(feature = "dist-client"))]
    let dist_command = None;
    #[cfg(feature = "dist-client")]
    let dist_command = Some(dist::CompileCommand {
        executable: path_transformer
            .as_dist(executable.canonicalize().unwrap().as_path())
            .ok_or_else(|| anyhow!("failed to transform executable path"))?,
        arguments: {
            let mut args = dist::osstrings_to_strings(&parsed_args.common_args)
                .ok_or_else(|| anyhow!("failed to transform arguments"))?;
            args.extend(vec![
                path_transformer
                    .as_dist(&parsed_args.input)
                    .ok_or_else(|| anyhow!("failed to transform input path"))?,
                "-o".into(),
                path_transformer
                    .as_dist(out_file)
                    .ok_or_else(|| anyhow!("failed to transform output path"))?,
            ]);
            args
        },
        env_vars: dist::osstring_tuples_to_strings(env_vars)
            .ok_or_else(|| anyhow!("failed to transform env vars"))?,
        cwd: path_transformer
            .as_dist_abs(cwd)
            .ok_or_else(|| anyhow!("failed to transform cwd"))?,
    });

    Ok((command, dist_command, Cacheable::Yes))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::compiler::*;
    use crate::test::utils::*;
    use std::path::PathBuf;

    fn parse_arguments_(arguments: Vec<String>) -> CompilerArguments<ParsedArguments> {
        let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        parse_arguments(&args, ".".as_ref())
    }

    #[test]
    fn test_parse_arguments_simple_wasm() {
        let args = stringvec!["input.wasm", "-O2", "-o", "output.wasm"];
        let ParsedArguments {
            input,
            language,
            outputs,
            common_args,
            ..
        } = match parse_arguments_(args) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert_eq!(Some("input.wasm"), input.to_str());
        assert_eq!(Language::Wasm, language);
        assert_map_contains!(
            outputs,
            (
                "obj",
                ArtifactDescriptor {
                    path: PathBuf::from(".").join("output.wasm"),
                    optional: false
                }
            )
        );
        assert_eq!(ovec!["-O2"], common_args);
    }

    #[test]
    fn test_parse_arguments_simple_wat() {
        let args = stringvec!["input.wat", "--strip-debug", "-o", "output.wasm"];
        let ParsedArguments {
            input,
            language,
            common_args,
            ..
        } = match parse_arguments_(args) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert_eq!(Some("input.wat"), input.to_str());
        assert_eq!(Language::Wasm, language);
        assert_eq!(ovec!["--strip-debug"], common_args);
    }

    #[test]
    fn test_parse_arguments_flags_affect_hash() {
        let args1 = stringvec!["input.wasm", "-O2", "-o", "output.wasm"];
        let args2 = stringvec!["input.wasm", "-O3", "-o", "output.wasm"];
        let parsed1 = match parse_arguments_(args1) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        let parsed2 = match parse_arguments_(args2) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        assert_ne!(parsed1.common_args, parsed2.common_args);
    }

    #[test]
    fn test_parse_arguments_requires_output() {
        let args = stringvec!["input.wasm", "-O2"];
        assert!(matches!(
            parse_arguments_(args),
            CompilerArguments::CannotCache("no output file", _)
        ));
    }

    #[test]
    fn test_parse_arguments_rejects_multiple_inputs() {
        let args = stringvec!["input.wasm", "extra.wasm", "-o", "output.wasm"];
        assert!(matches!(
            parse_arguments_(args),
            CompilerArguments::CannotCache("multiple input files", _)
        ));
    }

    #[test]
    fn test_parse_arguments_rejects_stdout_output() {
        let args = stringvec!["input.wasm", "-o", "-"];
        assert!(matches!(
            parse_arguments_(args),
            CompilerArguments::CannotCache("-o", _)
        ));
    }

    #[test]
    fn test_parse_arguments_rejects_response_files() {
        let args = stringvec!["@args.rsp"];
        assert!(matches!(
            parse_arguments_(args),
            CompilerArguments::CannotCache("@", _)
        ));
    }

    #[test]
    fn test_parse_arguments_rejects_stdin_input() {
        let args = stringvec!["-", "-o", "output.wasm"];
        assert!(matches!(
            parse_arguments_(args),
            CompilerArguments::CannotCache("stdin input", _)
        ));
    }

    #[test]
    fn test_generate_compile_commands() {
        let parsed_args = match parse_arguments_(stringvec![
            "input.wasm",
            "-O2",
            "--strip-debug",
            "-o",
            "output.wasm"
        ]) {
            CompilerArguments::Ok(args) => args,
            o => panic!("Got unexpected parse result: {:?}", o),
        };
        let f = TestFixture::new();
        let compiler = &f.bins[0];
        let mut path_transformer = dist::PathTransformer::new();
        let (command, _, cacheable) = generate_compile_commands(
            &mut path_transformer,
            compiler,
            &parsed_args,
            f.tempdir.path(),
            &[],
        )
        .unwrap();
        assert_eq!(
            command.arguments,
            ovec!["-O2", "--strip-debug", "input.wasm", "-o", ".\\output.wasm"]
        );
        assert_eq!(Cacheable::Yes, cacheable);
    }
}
