use std::{
	env, fs,
	path::{Path, PathBuf}
};

const ORT_ENV_SYSTEM_LIB_LOCATION: &str = "ORT_LIB_LOCATION";
const ORT_ENV_SYSTEM_LIB_PROFILE: &str = "ORT_LIB_PROFILE";
const ORT_ENV_PREFER_DYNAMIC_LINK: &str = "ORT_PREFER_DYNAMIC_LINK";
const ORT_ENV_CXX_STDLIB: &str = "ORT_CXX_STDLIB";
const ENV_CXXSTDLIB: &str = "CXXSTDLIB"; // Used by the `cc` crate - we should mirror if this is set for other C++ crates
#[cfg(feature = "download-binaries")]
const ORT_EXTRACT_DIR: &str = "onnxruntime";
// Lucidos patch (ADR 0595): the archive is fetched from this mirror before the
// upstream URL in dist.txt. The mirror path is the upstream path after
// `UPSTREAM_PATH_MARKER`, so dist.txt stays byte-identical to upstream.
#[cfg(feature = "download-binaries")]
const MIRROR_BASE_URL: &str = "https://github.com/lucidos-dev/onnxruntime-mirror/releases/download";
const ORT_ENV_MIRROR_URL: &str = "LUCIDOS_ORT_MIRROR_URL";
#[cfg(feature = "download-binaries")]
const UPSTREAM_PATH_MARKER: &str = "msort-binary/";
const DIST_TABLE: &str = include_str!("dist.txt");

#[path = "src/internal/dirs.rs"]
mod dirs;
use self::dirs::cache_dir;

#[cfg(feature = "download-binaries")]
#[path = "src/internal/download.rs"]
mod download;

#[cfg(feature = "download-binaries")]
fn fetch_once(source_url: &str) -> Result<Vec<u8>, download::FetchError> {
	let resp = ureq::AgentBuilder::new()
		.try_proxy_from_env(true)
		.timeout_connect(std::time::Duration::from_secs(30))
		.timeout_read(std::time::Duration::from_secs(60))
		.timeout(std::time::Duration::from_secs(600))
		.build()
		.get(source_url)
		.call()
		.map_err(|err| match err {
			ureq::Error::Status(status, _) => download::FetchError::http(status, err.to_string()),
			ureq::Error::Transport(error) => download::FetchError::transport(
				&error,
				matches!(error.kind(), ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed),
			),
		})?;

	let mut buffer = Vec::new();
	resp.into_reader().read_to_end(&mut buffer).map_err(download::FetchError::io)?;
	Ok(buffer)
}

#[cfg(feature = "download-binaries")]
fn mirror_url(upstream_url: &str) -> Option<String> {
	let base = env::var(ORT_ENV_MIRROR_URL).unwrap_or_else(|_| MIRROR_BASE_URL.to_owned());
	let (_, path) = upstream_url.split_once(UPSTREAM_PATH_MARKER)?;
	Some(format!("{}/{path}", base.trim_end_matches('/')))
}

/// Tries the mirror, then upstream. Only transient failures retry, with bounded backoff.
/// Every response must match dist.txt before it can reach extract_tgz.
#[cfg(feature = "download-binaries")]
fn fetch_verified(upstream_url: &str, hash: &str) -> Vec<u8> {
	let mirror = mirror_url(upstream_url);
	let sources = mirror.iter().cloned().chain([upstream_url.to_owned()]);
	download::fetch_sources(
		sources,
		fetch_once,
		|url, bytes| {
			let verified = verify_file(bytes, hash);
			if verified {
				let name = if mirror.as_deref() == Some(url) { "the Lucidos mirror" } else { "upstream" };
				println!("fetched ONNX Runtime from {name}: {url}");
			}
			verified
		},
		|seconds| std::thread::sleep(std::time::Duration::from_secs(seconds)),
	).unwrap_or_else(|failures| panic!(
		"could not download ONNX Runtime from the Lucidos mirror or from upstream:\n  {}\nBuild ONNX Runtime yourself and set `{ORT_ENV_SYSTEM_LIB_LOCATION}` to link it instead.",
		failures.join("\n  ")
	))
}

fn find_dist(target: &str, feature_set: &str) -> Option<(&'static str, &'static str)> {
	DIST_TABLE
		.split('\n')
		.filter(|c| !c.is_empty() && !c.starts_with('#'))
		.map(|c| c.split('\t').collect::<Vec<_>>())
		.find(|c| c[0] == feature_set && c[1] == target)
		.map(|c| (c[2], c[3]))
}

#[cfg(feature = "download-binaries")]
fn hex_str_to_bytes(c: impl AsRef<[u8]>) -> Vec<u8> {
	fn nibble(c: u8) -> u8 {
		match c {
			b'A'..=b'F' => c - b'A' + 10,
			b'a'..=b'f' => c - b'a' + 10,
			b'0'..=b'9' => c - b'0',
			_ => panic!()
		}
	}

	c.as_ref().chunks(2).map(|n| nibble(n[0]) << 4 | nibble(n[1])).collect()
}

#[cfg(feature = "download-binaries")]
fn verify_file(buf: &[u8], hash: impl AsRef<[u8]>) -> bool {
	<sha2::Sha256 as sha2::Digest>::digest(buf)[..] == hex_str_to_bytes(hash)
}

#[cfg(feature = "download-binaries")]
fn extract_tgz(buf: &[u8], output: &Path) {
	let buf: std::io::BufReader<&[u8]> = std::io::BufReader::new(buf);
	let tar = flate2::read::GzDecoder::new(buf);
	let mut archive = tar::Archive::new(tar);
	archive.unpack(output).expect("Failed to extract .tgz file");
}

#[cfg(feature = "copy-dylibs")]
fn copy_libraries(lib_dir: &Path, out_dir: &Path) {
	// get the target directory - we need to place the dlls next to the executable so they can be properly loaded by windows
	let out_dir = out_dir.ancestors().nth(3).unwrap();
	for out_dir in [out_dir.to_path_buf(), out_dir.join("examples"), out_dir.join("deps")] {
		#[cfg(windows)]
		let mut copy_fallback = false;
		#[cfg(not(windows))]
		let copy_fallback = false;

		let lib_files = std::fs::read_dir(lib_dir).unwrap_or_else(|_| panic!("Failed to read contents of `{}` (does it exist?)", lib_dir.display()));
		for lib_file in lib_files.filter(|e| {
			e.as_ref().ok().map_or(false, |e| {
				e.file_type().map_or(false, |e| !e.is_dir()) && [".dll", ".so", ".dylib"].into_iter().any(|v| e.path().to_string_lossy().contains(v))
			})
		}) {
			let lib_file = lib_file.unwrap();
			let lib_path = lib_file.path();
			let lib_name = lib_path.file_name().unwrap();
			let out_path = out_dir.join(lib_name);
			if !out_path.exists() {
				if out_path.is_symlink() {
					fs::remove_file(&out_path).unwrap();
				}
				#[cfg(windows)]
				if std::os::windows::fs::symlink_file(&lib_path, &out_path).is_err() {
					copy_fallback = true;
					std::fs::copy(&lib_path, &out_path).unwrap();
				}
				#[cfg(unix)]
				std::os::unix::fs::symlink(&lib_path, &out_path).unwrap();
			}
			if !copy_fallback {
				println!("cargo:rerun-if-changed={}", out_path.to_str().unwrap());
			}
		}

		// If we had to fallback to copying files on Windows, break early to avoid copying to 3 different directories
		if copy_fallback {
			break;
		}
	}
}

fn add_search_dir<P: AsRef<Path>>(base: P) {
	let base = base.as_ref();
	if base.join("Release").is_dir() {
		println!("cargo:rustc-link-search=native={}", base.join("Release").display());
	} else if base.join("Debug").is_dir() {
		println!("cargo:rustc-link-search=native={}", base.join("Debug").display());
	} else {
		println!("cargo:rustc-link-search=native={}", base.display());
	}
}

fn static_link_prerequisites(using_pyke_libs: bool) {
	let target_triple = env::var("TARGET").unwrap();

	let cpp_link_stdlib = if let Ok(stdlib) = env::var(ORT_ENV_CXX_STDLIB).or_else(|_| env::var(ENV_CXXSTDLIB)) {
		if stdlib.is_empty() { None } else { Some(stdlib) }
	} else if target_triple.contains("msvc") {
		None
	} else if target_triple.contains("apple") {
		Some("c++".to_string())
	} else if target_triple.contains("android") {
		Some("c++_shared".to_string())
	} else {
		Some("stdc++".to_string())
	};
	if let Some(cpp_link_stdlib) = cpp_link_stdlib {
		println!("cargo:rustc-link-lib={cpp_link_stdlib}");
	}

	if target_triple.contains("apple") {
		println!("cargo:rustc-link-lib=framework=Foundation");
	}
	if target_triple.contains("windows") && using_pyke_libs {
		println!("cargo:rustc-link-lib=dxguid");
		println!("cargo:rustc-link-lib=DXCORE");
		println!("cargo:rustc-link-lib=DXGI");
		println!("cargo:rustc-link-lib=D3D12");
		println!("cargo:rustc-link-lib=DirectML");
	}
}

fn prefer_dynamic_linking() -> bool {
	match env::var(ORT_ENV_PREFER_DYNAMIC_LINK) {
		Ok(val) => val == "1" || val.to_lowercase() == "true",
		Err(_) => cfg!(feature = "cuda") || cfg!(feature = "tensorrt")
	}
}

fn prepare_libort_dir() -> (PathBuf, bool) {
	if let Ok(base_lib_dir) = env::var(ORT_ENV_SYSTEM_LIB_LOCATION) {
		let base_lib_dir = PathBuf::from(base_lib_dir);

		let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap().to_lowercase();
		let platform_format_lib = |a: &str| {
			if target_os.contains("windows") { format!("{}.lib", a) } else { format!("lib{}.a", a) }
		};
		let optional_link_lib = |dir: &Path, lib: &str| {
			if dir.exists() && dir.join(platform_format_lib(lib)).exists() {
				add_search_dir(dir);
				println!("cargo:rustc-link-lib=static={lib}");
				true
			} else {
				false
			}
		};
		let vcpkg_target = match env::var("TARGET").as_deref() {
			Ok("i686-pc-windows-msvc") => Some("x86-windows"),
			Ok("x86_64-pc-windows-msvc") => Some("x64-windows"),
			Ok("x86_64-uwp-windows-msvc") => Some("x64-uwp"),
			Ok("aarch64-pc-windows-msvc") => Some("arm64-windows"),
			Ok("aarch64-uwp-windows-msvc") => Some("arm64-uwp"),
			Ok("aarch64-apple-darwin") => Some("arm64-osx"),
			Ok("x86_64-apple-darwin") => Some("x64-osx"),
			Ok("x86_64-unknown-linux-gnu") => Some("x64-linux"),
			Ok("armv7-linux-androideabi") => Some("arm-neon-android"),
			Ok("x86_64-linux-android") => Some("x64-android"),
			Ok("aarch64-linux-android") => Some("arm64-android"),
			_ => None
		};

		let mut profile = env::var(ORT_ENV_SYSTEM_LIB_PROFILE).unwrap_or_default();
		if profile.is_empty() {
			for i in ["Release", "RelWithDebInfo", "MinSizeRel", "Debug"] {
				if base_lib_dir.join(i).exists() && base_lib_dir.join(i).join(platform_format_lib("onnxruntime_common")).exists() {
					profile = String::from(i);
					break;
				}
			}
		}

		add_search_dir(&base_lib_dir);

		let mut needs_link = true;
		if base_lib_dir.join(platform_format_lib("onnxruntime")).exists() {
			println!("cargo:rustc-link-lib=static=onnxruntime");
			needs_link = false;
		} else if !prefer_dynamic_linking() {
			#[allow(clippy::type_complexity)]
			let static_configs: Vec<(PathBuf, PathBuf, PathBuf, Box<dyn Fn(PathBuf, &String) -> PathBuf>)> = vec![
				(base_lib_dir.join(&profile), base_lib_dir.join("lib"), base_lib_dir.join("_deps"), Box::new(|p: PathBuf, profile| p.join(profile))),
				(base_lib_dir.join(&profile), base_lib_dir.join("lib"), base_lib_dir.join(&profile).join("_deps"), Box::new(|p: PathBuf, _| p)),
				(base_lib_dir.clone(), base_lib_dir.join("lib"), base_lib_dir.parent().unwrap().join("_deps"), Box::new(|p: PathBuf, _| p)),
				(base_lib_dir.join("onnxruntime"), base_lib_dir.join("onnxruntime").join("lib"), base_lib_dir.join("_deps"), Box::new(|p: PathBuf, _| p)),
			];
			for (lib_dir, extension_lib_dir, external_lib_dir, transform_dep) in static_configs {
				if lib_dir.join(platform_format_lib("onnxruntime_common")).exists() && external_lib_dir.exists() {
					add_search_dir(&lib_dir);

					for lib in &["common", "flatbuffers", "framework", "graph", "lora", "mlas", "optimizer", "providers", "session", "util"] {
						let lib_path = lib_dir.join(platform_format_lib(&format!("onnxruntime_{lib}")));
						// sanity check, just make sure the library exists before we try to link to it
						if lib_path.exists() {
							println!("cargo:rustc-link-lib=static=onnxruntime_{lib}");
						} else {
							panic!("[ort] unable to find ONNX Runtime library: {}", lib_path.display());
						}
					}

					if extension_lib_dir.exists() && extension_lib_dir.join(platform_format_lib("ortcustomops")).exists() {
						add_search_dir(&extension_lib_dir);
						println!("cargo:rustc-link-lib=static=ortcustomops");
						println!("cargo:rustc-link-lib=static=ocos_operators");
						println!("cargo:rustc-link-lib=static=noexcep_operators");
					}

					let (vcpkg_lib_dir, has_vcpkg_link) = {
						let vcpkg_base_dir = base_lib_dir.join("vcpkg_installed");
						if let Some(vcpkg_target) = vcpkg_target {
							if vcpkg_base_dir.join(vcpkg_target).exists() {
								let vcpkg_lib_dir = vcpkg_base_dir.join(vcpkg_target).join("lib");
								add_search_dir(&vcpkg_lib_dir);
								(Some(vcpkg_lib_dir), true)
							} else {
								(None, false)
							}
						} else {
							(None, false)
						}
					};

					let protobuf_build = if !has_vcpkg_link {
						let protobuf_build = transform_dep(external_lib_dir.join("protobuf-build"), &profile);
						add_search_dir(&protobuf_build);
						protobuf_build
					} else {
						vcpkg_lib_dir.clone().unwrap()
					};
					for lib in ["protobuf-lited", "protobuf-lite", "protobuf"] {
						if target_os.contains("windows") && protobuf_build.join(platform_format_lib(&format!("lib{lib}"))).exists() {
							println!("cargo:rustc-link-lib=static=lib{lib}")
						} else if protobuf_build.join(platform_format_lib(lib)).exists() {
							println!("cargo:rustc-link-lib=static={lib}");
						}
					}

					add_search_dir(transform_dep(external_lib_dir.join("onnx-build"), &profile));
					println!("cargo:rustc-link-lib=static=onnx");
					println!("cargo:rustc-link-lib=static=onnx_proto");

					// some builds of ONNX Runtime, particularly the default no-EP windows build, don't require nsync
					if !has_vcpkg_link {
						optional_link_lib(&transform_dep(external_lib_dir.join("google_nsync-build"), &profile), "nsync_cpp");
					} else {
						optional_link_lib(vcpkg_lib_dir.as_ref().unwrap(), "nsync_cpp");
					}

					if !has_vcpkg_link {
						add_search_dir(transform_dep(external_lib_dir.join("pytorch_cpuinfo-build"), &profile));
						// clog isn't built when not building unit tests, or when compiling for android
						for potential_clog_path in [
							transform_dep(external_lib_dir.join("pytorch_cpuinfo-build").join("deps").join("clog"), &profile),
							transform_dep(external_lib_dir.join("pytorch_clog-build"), &profile)
						] {
							if optional_link_lib(&potential_clog_path, "clog") {
								break;
							}
						}
					} else {
						optional_link_lib(vcpkg_lib_dir.as_ref().unwrap(), "clog");
					}
					println!("cargo:rustc-link-lib=static=cpuinfo");

					if !has_vcpkg_link {
						add_search_dir(transform_dep(external_lib_dir.join("re2-build"), &profile));
					}
					println!("cargo:rustc-link-lib=static=re2");

					if has_vcpkg_link && target_os.contains("windows") {
						println!("cargo:rustc-link-lib=static=abseil_dll");
					} else {
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("debugging"), &profile));
						println!("cargo:rustc-link-lib=static=absl_examine_stack");
						println!("cargo:rustc-link-lib=static=absl_debugging_internal");
						println!("cargo:rustc-link-lib=static=absl_demangle_internal");
						println!("cargo:rustc-link-lib=static=absl_demangle_rust");
						println!("cargo:rustc-link-lib=static=absl_decode_rust_punycode");
						println!("cargo:rustc-link-lib=static=absl_utf8_for_code_point");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("base"), &profile));
						println!("cargo:rustc-link-lib=static=absl_base");
						println!("cargo:rustc-link-lib=static=absl_spinlock_wait");
						println!("cargo:rustc-link-lib=static=absl_malloc_internal");
						println!("cargo:rustc-link-lib=static=absl_strerror");
						println!("cargo:rustc-link-lib=static=absl_raw_logging_internal");
						println!("cargo:rustc-link-lib=static=absl_throw_delegate");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("hash"), &profile));
						println!("cargo:rustc-link-lib=static=absl_hash");
						println!("cargo:rustc-link-lib=static=absl_city");
						println!("cargo:rustc-link-lib=static=absl_low_level_hash");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("container"), &profile));
						println!("cargo:rustc-link-lib=static=absl_raw_hash_set");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("synchronization"), &profile));
						println!("cargo:rustc-link-lib=static=absl_kernel_timeout_internal");
						println!("cargo:rustc-link-lib=static=absl_graphcycles_internal");
						println!("cargo:rustc-link-lib=static=absl_synchronization");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("time"), &profile));
						println!("cargo:rustc-link-lib=static=absl_time_zone");
						println!("cargo:rustc-link-lib=static=absl_time");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("numeric"), &profile));
						println!("cargo:rustc-link-lib=static=absl_int128");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("strings"), &profile));
						println!("cargo:rustc-link-lib=static=absl_str_format_internal");
						println!("cargo:rustc-link-lib=static=absl_strings");
						println!("cargo:rustc-link-lib=static=absl_string_view");
						println!("cargo:rustc-link-lib=static=absl_strings_internal");
						add_search_dir(transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("debugging"), &profile));
						println!("cargo:rustc-link-lib=static=absl_symbolize");
						println!("cargo:rustc-link-lib=static=absl_stacktrace");
						let abseil_lib_log_dir = if !has_vcpkg_link {
							let dir = transform_dep(external_lib_dir.join("abseil_cpp-build").join("absl").join("log"), &profile);
							add_search_dir(&dir);
							dir
						} else {
							vcpkg_lib_dir.clone().unwrap()
						};
						println!("cargo:rustc-link-lib=static=absl_log_globals");
						println!("cargo:rustc-link-lib=static=absl_log_internal_format");
						println!("cargo:rustc-link-lib=static=absl_log_internal_proto");
						println!("cargo:rustc-link-lib=static=absl_log_internal_globals");
						optional_link_lib(&abseil_lib_log_dir, "absl_log_internal_check_op");
						println!("cargo:rustc-link-lib=static=absl_log_internal_log_sink_set");
						println!("cargo:rustc-link-lib=static=absl_log_sink");
						println!("cargo:rustc-link-lib=static=absl_log_internal_message");
					}

					// link static EPs if present
					// not sure if these are the right libs but they're optional links so...
					optional_link_lib(&lib_dir, "onnxruntime_providers_acl");
					optional_link_lib(&lib_dir, "onnxruntime_providers_armnn");
					optional_link_lib(&lib_dir, "onnxruntime_providers_azure");
					if optional_link_lib(&lib_dir, "onnxruntime_providers_coreml") {
						println!("cargo:rustc-link-lib=framework=CoreML");
						println!("cargo:rustc-link-lib=coreml_proto");
					}
					if optional_link_lib(&lib_dir, "onnxruntime_providers_dml") {
						println!("cargo:rustc-link-lib=dxguid");
						println!("cargo:rustc-link-lib=DXCORE");
						println!("cargo:rustc-link-lib=DXGI");
						println!("cargo:rustc-link-lib=D3D12");
						println!("cargo:rustc-link-lib=DirectML");
					}
					optional_link_lib(&lib_dir, "onnxruntime_providers_nnapi");
					optional_link_lib(&lib_dir, "onnxruntime_providers_qnn");
					optional_link_lib(&lib_dir, "onnxruntime_providers_rknpu");
					optional_link_lib(&lib_dir, "onnxruntime_providers_tvm");
					if optional_link_lib(&lib_dir, "onnxruntime_providers_xnnpack") {
						let xnnpack_build_dir = transform_dep(external_lib_dir.join("googlexnnpack-build"), &profile);
						add_search_dir(&xnnpack_build_dir);
						println!("cargo:rustc-link-lib=static=XNNPACK");
						optional_link_lib(&xnnpack_build_dir, "microkernels-prod");

						add_search_dir(transform_dep(external_lib_dir.join("pthreadpool-build"), &profile));
						println!("cargo:rustc-link-lib=static=pthreadpool");
					}

					needs_link = false;
					break;
				}
			}
			if needs_link {
				// none of the static link patterns matched, we might be trying to dynamic link so copy dylibs if requested
				#[cfg(feature = "copy-dylibs")]
				{
					let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
					if base_lib_dir.join("lib").is_dir() {
						copy_libraries(&base_lib_dir.join("lib"), &out_dir);
					} else if base_lib_dir.join(&profile).is_dir() {
						copy_libraries(&base_lib_dir.join(profile), &out_dir);
					}
				}
			}
		}

		(base_lib_dir, needs_link)
	} else {
		#[cfg(feature = "download-binaries")]
		{
			let target = env::var("TARGET").unwrap().to_string();

			let mut feature_set = Vec::new();
			if cfg!(feature = "training") {
				feature_set.push("train");
			}
			if cfg!(any(feature = "cuda", feature = "tensorrt")) {
				feature_set.push("cu12");
			} else if cfg!(feature = "rocm") {
				feature_set.push("rocm");
			}
			let feature_set = if !feature_set.is_empty() { feature_set.join(",") } else { "none".to_owned() };
			println!("selected feature set: {feature_set}");
			let mut dist = find_dist(&target, &feature_set);
			if dist.is_none() && feature_set != "none" {
				dist = find_dist(&target, "none");
			}

			if dist.is_none() {
				panic!(
					"downloaded binaries not available for target {target}{}\nyou may have to compile ONNX Runtime from source",
					if feature_set != "none" {
						format!(" (note: also requested features `{feature_set}`)")
					} else {
						String::new()
					}
				);
			}

			let (prebuilt_url, prebuilt_hash) = dist.unwrap();

			let mut cache_dir = cache_dir()
				.expect("could not determine cache directory")
				.join("dfbin")
				.join(target)
				.join(prebuilt_hash);
			if fs::create_dir_all(&cache_dir).is_err() {
				cache_dir = env::var("OUT_DIR").unwrap().into();
			}

			let lib_dir = cache_dir.join(ORT_EXTRACT_DIR);
			if !lib_dir.exists() {
				let downloaded_file = fetch_verified(prebuilt_url, prebuilt_hash);
				extract_tgz(&downloaded_file, &cache_dir);
			}

			static_link_prerequisites(true);

			#[cfg(feature = "copy-dylibs")]
			{
				let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
				copy_libraries(&lib_dir.join("lib"), &out_dir);
			}

			(lib_dir, true)
		}
		#[cfg(not(feature = "download-binaries"))]
		{
			println!("cargo:rustc-link-lib=add_ort_library_path_or_enable_feature_download-binaries_see_ort_docs");
			(PathBuf::default(), false)
		}
	}
}

fn real_main(link: bool) {
	println!("cargo:rerun-if-env-changed={}", ORT_ENV_SYSTEM_LIB_LOCATION);
	println!("cargo:rerun-if-env-changed={}", ORT_ENV_SYSTEM_LIB_PROFILE);
	println!("cargo:rerun-if-env-changed={}", ORT_ENV_PREFER_DYNAMIC_LINK);
	println!("cargo:rerun-if-env-changed={}", ORT_ENV_CXX_STDLIB);
	println!("cargo:rerun-if-env-changed={}", ENV_CXXSTDLIB);
	println!("cargo:rerun-if-env-changed={ORT_ENV_MIRROR_URL}");

	let (install_dir, needs_link) = prepare_libort_dir();

	let lib_dir = if install_dir.join("lib").exists() { install_dir.join("lib") } else { install_dir };

	if link {
		if needs_link {
			let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
			let static_lib_file_name = if target_os.contains("windows") { "onnxruntime.lib" } else { "libonnxruntime.a" };

			let static_lib_path = lib_dir.join(static_lib_file_name);
			if !prefer_dynamic_linking() && static_lib_path.exists() {
				println!("cargo:rustc-link-lib=static=onnxruntime");
			} else {
				println!("cargo:rustc-link-lib=onnxruntime");
			}
			println!("cargo:rustc-link-search=native={}", lib_dir.display());
		}

		static_link_prerequisites(false);
	}
}

fn main() {
	if env::var("DOCS_RS").is_ok() {
		return;
	}

	// Lucidos patch (ADR 0595): no pkg-config probe, so every build links the pinned static library.
	if cfg!(feature = "load-dynamic") {
		// All we do is download the binaries and place them in the output directory, without linking them.
		if cfg!(feature = "download-binaries") && env::var(ORT_ENV_SYSTEM_LIB_LOCATION).is_err() {
			real_main(false);
		}
	} else {
		real_main(true);
	}
}
