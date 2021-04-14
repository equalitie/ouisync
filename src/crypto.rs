// TODO: crypto demonstration - to be deleted
#[test]
fn demo() {
    use chacha20poly1305::{
        aead::{
            generic_array::GenericArray,
            stream::{DecryptorLE31, EncryptorLE31},
            NewAead, Payload,
        },
        ChaCha20Poly1305, Key,
    };

    let blocks_plain_original = vec![
        (b"foo".to_vec(), b"Lorem ipsum dolor sit amet".to_vec()),
        (b"bar".to_vec(), b"consectetur adipiscing elit".to_vec()),
        (b"baz".to_vec(), b"sed do eiusmod tempor".to_vec()),
    ];

    // Key size is 32 bytes.
    let key = Key::from_slice(b"a very secret key...............");

    // Nonce size is the nonce size of the underlying cipher minus nonce overhead of the
    // stream primitive. In this case it is 12 - 4 = 8
    let nonce = GenericArray::from_slice(b"a nonce.");

    // Encrypt

    let mut encryptor = EncryptorLE31::from_aead(ChaCha20Poly1305::new(key), nonce);
    let mut blocks_cipher = Vec::new();

    for (key_plain, value_plain) in &blocks_plain_original[..blocks_plain_original.len() - 1] {
        let value_cipher = encryptor
            .encrypt_next(Payload {
                msg: value_plain,
                aad: key_plain,
            })
            .unwrap(); // NOTE: this panics if we exhaust the counter*. The counter's max value is
                       //       2^31 so it's unlikely to happen in practice, but we should still
                       //       probably error gracefully instead of crashing even in this unlikely
                       //       case.
                       //
                       //       *) also when the plaintext block or the associated data is too long,
                       //          but we can easily control that.
        blocks_cipher.push((key_plain.clone(), value_cipher));
    }

    // Last block
    let (key_plain, value_plain) = &blocks_plain_original[blocks_plain_original.len() - 1];
    let value_cipher = encryptor
        .encrypt_last(Payload {
            msg: value_plain,
            aad: key_plain,
        })
        .unwrap();
    blocks_cipher.push((key_plain.clone(), value_cipher));

    // Decrypt

    let mut decryptor = DecryptorLE31::from_aead(ChaCha20Poly1305::new(key), nonce);
    let mut blocks_plain_decrypted = Vec::new();

    for (key_plain, value_cipher) in &blocks_cipher[..blocks_plain_original.len() - 1] {
        let value_plain = decryptor
            .decrypt_next(Payload {
                msg: value_cipher,
                aad: key_plain,
            })
            .unwrap();
        blocks_plain_decrypted.push((key_plain.clone(), value_plain));
    }

    // Last block
    let (key_plain, value_cipher) = &blocks_cipher[blocks_cipher.len() - 1];
    let value_plain = decryptor
        .decrypt_last(Payload {
            msg: value_cipher,
            aad: key_plain,
        })
        .unwrap();
    blocks_plain_decrypted.push((key_plain.clone(), value_plain));

    assert_eq!(blocks_plain_original, blocks_plain_decrypted);
}
