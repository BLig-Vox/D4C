use crate::store::Result;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, VerifyingKey};
use rand::{RngCore, rngs::OsRng};
use serde_json::Value;
use sha2::{Digest, Sha256};
pub fn nonce() -> String {
    let mut b = [0; 32];
    OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}
pub fn challenge_text(host: &str, nonce: &str) -> String {
    format!("d4c-service-v1|{host}|{nonce}")
}
pub fn verify(host: &str, nonce: &str, proof: &Value) -> Result<(String, String)> {
    let public = proof["publicKey"].as_str().ok_or("publicKey required")?;
    if public.len() > 64 {
        return Err("invalid public key".into());
    }
    let b: [u8; 32] = URL_SAFE_NO_PAD
        .decode(public)
        .map_err(|_| "invalid public key")?
        .try_into()
        .map_err(|_| "invalid public key")?;
    let id = format!("{:x}", Sha256::digest(b));
    if proof["deviceId"].as_str() != Some(id.as_str()) {
        return Err("device ID does not match key".into());
    }
    let signature = proof["signature"].as_str().ok_or("signature required")?;
    if signature.len() > 128 {
        return Err("invalid signature".into());
    }
    let signature = Signature::from_slice(
        &URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| "invalid signature")?,
    )
    .map_err(|_| "invalid signature")?;
    VerifyingKey::from_bytes(&b)
        .map_err(|_| "invalid public key")?
        .verify_strict(challenge_text(host, nonce).as_bytes(), &signature)
        .map_err(|_| "invalid proof")?;
    Ok((id, public.to_string()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;
    #[test]
    fn proof_is_bound_to_server_and_one_challenge() {
        let key = SigningKey::generate(&mut OsRng);
        let public = key.verifying_key().to_bytes();
        let n = nonce();
        let host = "gateway.example";
        let signature = key.sign(challenge_text(host, &n).as_bytes());
        let proof = json!({"deviceId":format!("{:x}",Sha256::digest(public)),"publicKey":URL_SAFE_NO_PAD.encode(public),"signature":URL_SAFE_NO_PAD.encode(signature.to_bytes())});
        assert!(verify(host, &n, &proof).is_ok());
        assert!(verify("other.example", &n, &proof).is_err());
        assert!(verify(host, &nonce(), &proof).is_err());
        let mut other = proof.clone();
        other["deviceId"] = json!("wrong");
        assert!(verify(host, &n, &other).is_err());
    }
}
