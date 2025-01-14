use flo_util::binary::*;
use flo_util::{BinDecode, BinEncode};

use crate::protocol::constants::{MessageType, PacketTypeId};
use crate::protocol::packet::PacketPayload;

#[derive(Debug, BinDecode, BinEncode, PartialEq)]
pub struct ChatToHost {
  pub to_players_len: u8,
  #[bin(repeat = "to_players_len")]
  pub to_players: Vec<u8>,
  pub from_player: u8,
  pub message: ChatMessage,
}

impl ChatToHost {
  pub fn is_in_game_chat(&self) -> bool {
    if let ChatMessage::Scoped { .. } = self.message {
      true
    } else {
      false
    }
  }

  pub fn chat_message(&self) -> Option<&[u8]> {
    match self.message {
      ChatMessage::Scoped { ref message, .. } => Some(message.as_bytes()),
      _ => None,
    }
  }

  pub fn lobby(from: u8, to: &[u8], message: impl IntoCStringLossy) -> Self {
    ChatToHost {
      to_players_len: to.len() as u8,
      to_players: to.to_vec(),
      from_player: from,
      message: ChatMessage::Chat(message.into_c_string_lossy()),
    }
  }

  pub fn in_game(scope: MessageScope, from: u8, to: &[u8], message: impl IntoCStringLossy) -> Self {
    ChatToHost {
      to_players_len: to.len() as u8,
      to_players: to.to_vec(),
      from_player: from,
      message: ChatMessage::Scoped {
        scope,
        message: message.into_c_string_lossy(),
      },
    }
  }
}

impl PacketPayload for ChatToHost {
  const PACKET_TYPE_ID: PacketTypeId = PacketTypeId::ChatToHost;
}

#[derive(Debug, PartialEq, Clone)]
pub enum ChatMessage {
  Chat(CString),
  TeamChange(u8),
  ColorChange(u8),
  RaceChange(u8),
  HandicapChange(u8),
  Scoped {
    scope: MessageScope,
    message: CString,
  },
}

impl BinDecode for ChatMessage {
  const MIN_SIZE: usize = 2;
  const FIXED_SIZE: bool = false;

  fn decode<T: Buf>(buf: &mut T) -> Result<Self, BinDecodeError> {
    buf.check_size(Self::MIN_SIZE)?;
    match MessageType::decode(buf)? {
      MessageType::Chat => Ok(ChatMessage::Chat(CString::decode(buf)?)),
      MessageType::TeamChange => Ok(ChatMessage::TeamChange(buf.get_u8())),
      MessageType::ColorChange => Ok(ChatMessage::ColorChange(buf.get_u8())),
      MessageType::RaceChange => Ok(ChatMessage::RaceChange(buf.get_u8())),
      MessageType::HandicapChange => Ok(ChatMessage::HandicapChange(buf.get_u8())),
      MessageType::Scoped => {
        let scope = MessageScope::decode(buf)?;
        let message = CString::decode(buf)?;
        Ok(ChatMessage::Scoped { scope, message })
      }
      MessageType::UnknownValue(v) => Err(BinDecodeError::failure(format!(
        "invalid message type: {}",
        v
      ))),
    }
  }
}

impl BinEncode for ChatMessage {
  fn encode<T: BufMut>(&self, buf: &mut T) {
    match *self {
      ChatMessage::Chat(ref msg) => {
        MessageType::Chat.encode(buf);
        msg.encode(buf);
      }
      ChatMessage::TeamChange(v) => {
        MessageType::TeamChange.encode(buf);
        buf.put_u8(v)
      }
      ChatMessage::ColorChange(v) => {
        MessageType::ColorChange.encode(buf);
        buf.put_u8(v)
      }
      ChatMessage::RaceChange(v) => {
        MessageType::RaceChange.encode(buf);
        buf.put_u8(v)
      }
      ChatMessage::HandicapChange(v) => {
        MessageType::HandicapChange.encode(buf);
        buf.put_u8(v)
      }
      ChatMessage::Scoped { scope, ref message } => {
        MessageType::Scoped.encode(buf);
        scope.encode(buf);
        message.encode(buf);
      }
    }
  }
}

impl ChatMessage {
  pub fn encode_len(&self) -> usize {
    1 + match *self {
      ChatMessage::Chat(ref msg) => msg.as_bytes_with_nul().len(),
      ChatMessage::TeamChange(_v) => size_of::<u8>(),
      ChatMessage::ColorChange(_v) => size_of::<u8>(),
      ChatMessage::RaceChange(_v) => size_of::<u8>(),
      ChatMessage::HandicapChange(_v) => size_of::<u8>(),
      ChatMessage::Scoped {
        scope: _,
        ref message,
      } => MessageScope::MIN_SIZE + message.as_bytes_with_nul().len(),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageScope {
  All,
  Allies,
  Observers,
  Player(u8),
}

impl BinDecode for MessageScope {
  const MIN_SIZE: usize = size_of::<u32>();
  const FIXED_SIZE: bool = true;

  fn decode<T: Buf>(buf: &mut T) -> Result<Self, BinDecodeError> {
    buf.check_size(Self::MIN_SIZE)?;
    match buf.get_u32_le() {
      0x00 => Ok(Self::All),
      0x01 => Ok(Self::Allies),
      0x02 => Ok(Self::Observers),
      n if n <= (u8::MAX - 0x03) as u32 => Ok(Self::Player((n - 0x03) as u8)),
      n => Err(BinDecodeError::failure(format!(
        "invalid chat message scope value: {}",
        n
      ))),
    }
  }
}

impl BinEncode for MessageScope {
  fn encode<T: BufMut>(&self, buf: &mut T) {
    buf.put_u32_le(match *self {
      Self::All => 0x00,
      Self::Allies => 0x01,
      Self::Observers => 0x02,
      Self::Player(v) => 0x02 + v as u32,
    });
  }
}

#[derive(Debug, BinEncode, BinDecode, PartialEq)]
pub struct ChatFromHost(pub ChatToHost);

impl From<ChatToHost> for ChatFromHost {
  fn from(inner: ChatToHost) -> Self {
    Self(inner)
  }
}

impl ChatFromHost {
  pub fn from_player(&self) -> u8 {
    self.0.from_player
  }

  pub fn lobby(from: u8, to: &[u8], message: impl IntoCStringLossy) -> Self {
    ChatToHost::lobby(from, to, message).into()
  }

  pub fn private_to_self(player_id: u8, message: impl IntoCStringLossy) -> Self {
    ChatToHost::in_game(
      MessageScope::Player(player_id),
      player_id,
      &[player_id],
      message,
    )
    .into()
  }
}

impl PacketPayload for ChatFromHost {
  const PACKET_TYPE_ID: PacketTypeId = PacketTypeId::ChatFromHost;
}

#[derive(Debug, BinEncode, BinDecode, PartialEq)]
pub struct ChatFromOthers(ChatToHost);

impl ChatFromOthers {
  pub fn new(msg: ChatToHost) -> Self {
    Self(msg)
  }
}

impl PacketPayload for ChatFromOthers {
  const PACKET_TYPE_ID: PacketTypeId = PacketTypeId::ChatFromOthers;
}
