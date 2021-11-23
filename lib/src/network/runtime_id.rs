define_array_wrapper! {
    /// Randomly generated ephemeral id that uniquely identifies a replica. Changes every time the
    /// replica is restarted.
    pub(super) struct RuntimeId([u8; 16]);
}
derive_rand_for_wrapper!(RuntimeId);
