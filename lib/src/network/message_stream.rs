// use super::{
//     message::{Content, Message},
//     object_stream::TypedObjectRead,
// };
// use crate::repository::PublicRepositoryId;
// use futures_util::stream::SelectAll;
// use std::{
//     collections::{HashMap, VecDeque},
//     sync::Arc,
// };
// use tokio::{net::tcp, sync::Mutex};

// pub(super) struct MessageMultiplexer {
//     stream: SelectAll<TypedObjectRead<Message, tcp::OwnedReadHalf>>,
// }

// impl MessageMultiplexer {
//     pub fn new() -> Self {

//     }

//     pub fn add(&mut self, stream: )
// }
