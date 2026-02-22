use asspp_core::plist_util;
use asspp_core::sinf::{self, InjectionSource};
use asspp_core::types::Sinf;
use base64::Engine;
use std::io::{Read, Seek, Write};

/// Inject SINFs and optional iTunesMetadata into an IPA file.
pub async fn inject(
  sinfs: &[Sinf],
  ipa_path: &str,
  itunes_metadata_b64: Option<&str>,
) -> Result<(), String> {
  let path = ipa_path.to_string();
  let sinfs = sinfs.to_vec();
  let metadata = itunes_metadata_b64.map(String::from);

  // Run ZIP operations in blocking task (CPU + I/O bound)
  tokio::task::spawn_blocking(move || inject_sync(&sinfs, &path, metadata.as_deref()))
    .await
    .map_err(|e| format!("Spawn error: {}", e))?
}

fn inject_sync(sinfs: &[Sinf], ipa_path: &str, itunes_metadata_b64: Option<&str>) -> Result<(), String> {
  let file = std::fs::File::open(ipa_path).map_err(|e| format!("Open IPA: {}", e))?;
  let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("Read ZIP: {}", e))?;

  // Find bundle name
  let bundle_name = find_bundle_name(&mut zip)?;

  // Determine injection source
  let source = find_injection_source(&mut zip, &bundle_name)?;

  // Decode sinf data
  let sinf_data: Vec<(i64, Vec<u8>)> = sinfs
    .iter()
    .map(|s| {
      let data = base64::engine::general_purpose::STANDARD
        .decode(&s.sinf)
        .map_err(|e| format!("Decode sinf: {}", e))?;
      Ok((s.id, data))
    })
    .collect::<Result<Vec<_>, String>>()?;

  // Prepare iTunesMetadata binary plist
  let metadata_binary = if let Some(b64) = itunes_metadata_b64 {
    let xml_bytes = base64::engine::general_purpose::STANDARD
      .decode(b64)
      .map_err(|e| format!("Decode metadata: {}", e))?;
    let xml_str = String::from_utf8_lossy(&xml_bytes);
    match plist_util::xml_to_binary_plist(&xml_str) {
      Ok(binary) => Some(binary),
      Err(_) => Some(xml_bytes), // Fallback: inject as-is
    }
  } else {
    None
  };

  // Plan injection
  let plan = sinf::plan_injection(
    &bundle_name,
    &source,
    &sinf_data,
    metadata_binary.as_deref(),
  );

  if plan.files.is_empty() {
    return Ok(());
  }

  // Rewrite the ZIP with injected files
  rewrite_zip(ipa_path, &mut zip, &plan)?;

  Ok(())
}

fn find_bundle_name<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> Result<String, String> {
  for i in 0..zip.len() {
    let entry = zip.by_index_raw(i).map_err(|e| format!("Read entry: {}", e))?;
    if let Some(name) = sinf::extract_bundle_name(entry.name()) {
      return Ok(name);
    }
  }
  Err("Could not read bundle name".into())
}

fn find_injection_source<R: Read + Seek>(
  zip: &mut zip::ZipArchive<R>,
  _bundle_name: &str,
) -> Result<InjectionSource, String> {
  // Try Manifest.plist first
  for i in 0..zip.len() {
    let entry = zip.by_index_raw(i).map_err(|e| format!("Read entry: {}", e))?;
    if sinf::is_manifest_plist(entry.name()) {
      drop(entry);
      let mut entry = zip.by_index(i).map_err(|e| format!("Read entry: {}", e))?;
      let mut buf = Vec::new();
      entry.read_to_end(&mut buf).map_err(|e| format!("Read manifest: {}", e))?;
      if let Some(val) = plist_util::parse_plist(&buf) {
        if let Some(paths) = plist_util::get_string_array(&val, "SinfPaths") {
          return Ok(InjectionSource::Manifest { sinf_paths: paths });
        }
      }
      break;
    }
  }

  // Fall back to Info.plist
  for i in 0..zip.len() {
    let entry = zip.by_index_raw(i).map_err(|e| format!("Read entry: {}", e))?;
    let name = entry.name().to_string();
    if sinf::is_info_plist(&name) {
      drop(entry);
      let mut entry = zip.by_index(i).map_err(|e| format!("Read entry: {}", e))?;
      let mut buf = Vec::new();
      entry.read_to_end(&mut buf).map_err(|e| format!("Read info: {}", e))?;
      if let Some(val) = plist_util::parse_plist(&buf) {
        if let Some(exec) = plist_util::get_string(&val, "CFBundleExecutable") {
          return Ok(InjectionSource::Info {
            bundle_executable: exec,
          });
        }
      }
      break;
    }
  }

  Err("Could not read manifest or info plist".into())
}

fn rewrite_zip<R: Read + Seek>(
  ipa_path: &str,
  zip: &mut zip::ZipArchive<R>,
  plan: &sinf::InjectionPlan,
) -> Result<(), String> {
  let tmp_path = format!("{}.tmp", ipa_path);

  {
    let out_file = std::fs::File::create(&tmp_path).map_err(|e| format!("Create temp: {}", e))?;
    let mut out_zip = zip::ZipWriter::new(out_file);

    let inject_paths: std::collections::HashSet<&str> =
      plan.files.iter().map(|(p, _)| p.as_str()).collect();

    // Copy existing entries, skipping ones we're replacing
    for i in 0..zip.len() {
      let entry = zip.by_index_raw(i).map_err(|e| format!("Read entry: {}", e))?;
      let name = entry.name().to_string();

      if inject_paths.contains(name.as_str()) {
        continue;
      }

      out_zip
        .raw_copy_file(entry)
        .map_err(|e| format!("Copy entry '{}': {}", name, e))?;
    }

    // Add injected files
    let options = zip::write::SimpleFileOptions::default()
      .compression_method(zip::CompressionMethod::Stored);

    for (path, data) in &plan.files {
      out_zip
        .start_file(path, options)
        .map_err(|e| format!("Start file '{}': {}", path, e))?;
      out_zip
        .write_all(data)
        .map_err(|e| format!("Write '{}': {}", path, e))?;
    }

    out_zip.finish().map_err(|e| format!("Finish ZIP: {}", e))?;
  }

  // Replace original with temp
  std::fs::rename(&tmp_path, ipa_path).map_err(|e| format!("Rename: {}", e))?;

  Ok(())
}
