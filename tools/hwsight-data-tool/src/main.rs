//! Developer tool for hwsight data releases. Not shipped to users.
//!
//!   hwsight-data-tool keygen <folder> <name>     make a signing key (<name>.key, private)
//!   hwsight-data-tool release <data-folder> <key> write and sign manifest.toml
//!   hwsight-data-tool verify <data-folder>        check a release against the built-in keys
//!
//! Private keys are 32 random bytes in a file only the owner can read. Never commit them.

use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::ExitCode;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hwsight_core::update;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["keygen", dir, name] => keygen(Path::new(dir), name),
        ["release", dir, key] => release(Path::new(dir), Path::new(key)),
        ["verify", dir] => verify(Path::new(dir)),
        _ => Err(
            "usage: keygen <folder> <name> | release <data-folder> <key> | verify <data-folder>"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(dir: &Path, name: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{name}.key"));
    if path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite a key",
            path.display()
        ));
    }
    let mut seed = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut seed))
        .map_err(|e| e.to_string())?;
    let key = SigningKey::from_bytes(&seed);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| e.to_string())?;
    std::io::Write::write_all(&mut file, &seed).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    let public = key.verifying_key().to_bytes();
    println!("private key: {} (keep secret, back it up)", path.display());
    println!(
        "public key for update::TRUSTED_KEYS:\n    {},",
        rust_array(&public)
    );
    Ok(())
}

fn release(dir: &Path, key_path: &Path) -> Result<(), String> {
    let seed: [u8; 32] = fs::read(key_path)
        .map_err(|e| format!("{}: {e}", key_path.display()))?
        .try_into()
        .map_err(|_| "a key file must be exactly 32 bytes".to_owned())?;
    let key = SigningKey::from_bytes(&seed);

    let mut manifest =
        String::from("# hwsight data release. Signed: see manifest.toml.sig.\nformat = 1\n");
    for name in update::known_files() {
        let path = dir.join(name);
        let Ok(bytes) = fs::read(&path) else { continue };
        let version = update::file_version(name, &bytes)?;
        manifest.push_str(&format!(
            "\n[[file]]\nname = \"{name}\"\nversion = \"{version}\"\nsize = {}\nsha256 = \"{}\"\n",
            bytes.len(),
            update::checksum(&bytes)
        ));
        println!("{name:14} {version}  {} bytes", bytes.len());
    }
    let signature = key.sign(manifest.as_bytes());
    fs::write(dir.join("manifest.toml"), &manifest).map_err(|e| e.to_string())?;
    fs::write(dir.join("manifest.toml.sig"), signature.to_bytes()).map_err(|e| e.to_string())?;
    let public = key.verifying_key().to_bytes();
    if !update::TRUSTED_KEYS.contains(&public) {
        eprintln!("warning: this key isn't in update::TRUSTED_KEYS, so hwsight won't accept it");
    }
    println!("signed manifest.toml with key {}", hex(&public));
    Ok(())
}

fn verify(dir: &Path) -> Result<(), String> {
    let manifest = fs::read(dir.join("manifest.toml")).map_err(|e| e.to_string())?;
    let sig = fs::read(dir.join("manifest.toml.sig")).map_err(|e| e.to_string())?;
    let sig = Signature::from_slice(&sig).map_err(|e| e.to_string())?;
    let ok = update::TRUSTED_KEYS.iter().any(|k| {
        VerifyingKey::from_bytes(k).is_ok_and(|k| k.verify_strict(&manifest, &sig).is_ok())
    });
    if ok {
        println!("signature ok");
        Ok(())
    } else {
        Err("signature doesn't match any built-in key".into())
    }
}

fn rust_array(bytes: &[u8]) -> String {
    let items: Vec<String> = bytes.iter().map(|b| format!("0x{b:02x}")).collect();
    format!("[{}]", items.join(", "))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
