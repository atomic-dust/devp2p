use bigint::{H256, H512};
use secp256k1::key::PublicKey;
use secp256k1::{self, SECP256K1};
use sha3::{Digest, Keccak256};

pub fn keccak256(data: &[u8]) -> H256 {
    let mut hasher = Keccak256::new();
    hasher.input(data);
    let out = hasher.result();
    H256::from(out.as_ref())
}

pub fn pk2id(pk: &PublicKey) -> H512 {
    let v = pk.serialize_vec(&SECP256K1, false);
    debug_assert!(v.len() == 65);
    H512::from(&v[1..])
}

pub fn id2pk(id: H512) -> Result<PublicKey, secp256k1::Error> {
    let s: [u8; 64] = id.into();
    let mut sp: Vec<u8> = s.as_ref().into();
    let mut r = vec![0x04u8];
    r.append(&mut sp);
    PublicKey::from_slice(&SECP256K1, r.as_ref())
}
