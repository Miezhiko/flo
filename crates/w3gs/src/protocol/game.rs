use flo_util::binary::*;
use flo_util::{BinDecode, BinEncode};

use crate::protocol::constants::{GameSettingFlags, PacketTypeId};
use crate::protocol::packet::PacketPayload;

#[derive(Debug, PartialEq, Clone)]
pub struct GameSettings {
  pub game_setting_flags: GameSettingFlags,
  pub unk_1: u8,
  pub map_width: u16,
  pub map_height: u16,
  pub map_checksum: u32,
  pub map_path: CString,
  pub host_name: CString,
  pub map_sha1: [u8; 20],
}

#[derive(Debug)]
pub struct GameSettingsMap {
  pub path: String,
  pub width: u16,
  pub height: u16,
  pub sha1: [u8; 20],
  pub checksum: u32,
}

impl GameSettings {
  pub fn new(flags: GameSettingFlags, map: GameSettingsMap) -> Self {
    Self {
      game_setting_flags: flags,
      unk_1: 0,
      map_width: map.width,
      map_height: map.height,
      map_checksum: map.checksum,
      map_sha1: map.sha1,
      map_path: map.path.into_c_string_lossy(),
      host_name: CString::new("FLO").unwrap(),
    }
  }

  fn get_encode_size(&self) -> usize {
    size_of::<u32>() /* Flags */
    + 1 /* 0x0 */
    + size_of::<u16>() /* Map width */
    + size_of::<u16>() /* Map height */
    + size_of::<u32>() /* Map checksum */
    + self.map_path.as_bytes().len() + 1 /* Map path */
    + self.host_name.as_bytes().len() + 1 /* Host name */
    + 1 /* 0x0 */
    + 20 /* Map Sha1 hash */
  }
}

impl BinEncode for GameSettings {
  fn encode<T: BufMut>(&self, buf: &mut T) {
    let len = self.get_encode_size();
    let mut stat_string_buf = Vec::<u8>::with_capacity(len);
    stat_string_buf.put_u32_le(self.game_setting_flags.bits());
    stat_string_buf.put_u8(self.unk_1);
    stat_string_buf.put_u16_le(self.map_width);
    stat_string_buf.put_u16_le(self.map_height);
    stat_string_buf.put_u32_le(self.map_checksum);
    stat_string_buf.put(self.map_path.as_bytes());
    stat_string_buf.put_u8(0);
    stat_string_buf.put(self.host_name.as_bytes());
    stat_string_buf.put_u8(0);
    stat_string_buf.put_u8(0);
    stat_string_buf.put(&self.map_sha1 as &[u8]);
    let encoded = flo_util::stat_string::encode(&stat_string_buf);
    buf.put_slice(&encoded);
    buf.put_u8(0);
  }
}

impl BinDecode for GameSettings {
  fn decode<T: Buf>(buf: &mut T) -> Result<Self, BinDecodeError> {
    let min_len = size_of::<u32>() /* Flags */
      + 1 /* 0x0 */
      + size_of::<u16>() /* Map width */
      + size_of::<u16>() /* Map height */
      + size_of::<u32>() /* Map xoro */
      + "x.w3m".len() + 1 /* Map path */
      + "h".len() + 1 /* Host name */
      + 1 /* 0x0 */
      + 20 /* Map Sha1 hash */;

    if buf.remaining() < flo_util::stat_string::encoded_len(min_len) {
      return Err(BinDecodeError::incomplete());
    }

    let cstr = CString::decode(buf)?;
    let data = flo_util::stat_string::decode(cstr.as_bytes());

    let mut buf = &data[..];

    let game_setting_flags = buf.get_u32_le();
    let game_setting_flags = GameSettingFlags::from_bits(game_setting_flags).ok_or_else(|| {
      BinDecodeError::failure(format!(
        "unknown game flags value: 0x{:x}",
        game_setting_flags
      ))
    })?;

    let unk_1 = buf.get_u8();

    let map_width = buf.get_u16_le();
    let map_height = buf.get_u16_le();
    let map_xoro = buf.get_u32_le();
    let map_path = CString::decode(&mut buf)?;
    let host_name = CString::decode(&mut buf)?;

    if buf.remaining() != 1 + 20 {
      return Err(BinDecodeError::incomplete());
    }

    let zero = buf.get_u8();

    if zero != 0 {
      return Err(BinDecodeError::failure("zero byte expected"));
    }

    let mut map_sha1 = [0; 20];
    buf.copy_to_slice(&mut map_sha1);

    Ok(GameSettings {
      game_setting_flags,
      unk_1,
      map_width,
      map_height,
      map_checksum: map_xoro,
      map_path,
      host_name,
      map_sha1,
    })
  }
}

#[derive(Debug, BinDecode, BinEncode, PartialEq)]
pub struct CountDownStart;

impl PacketPayload for CountDownStart {
  const PACKET_TYPE_ID: PacketTypeId = PacketTypeId::CountDownStart;
}

#[derive(Debug, BinDecode, BinEncode, PartialEq)]
pub struct CountDownEnd;

impl PacketPayload for CountDownEnd {
  const PACKET_TYPE_ID: PacketTypeId = PacketTypeId::CountDownEnd;
}

#[derive(Debug, BinDecode, BinEncode, PartialEq)]
pub struct GameLoadedSelf;

impl PacketPayload for GameLoadedSelf {
  const PACKET_TYPE_ID: PacketTypeId = PacketTypeId::GameLoadedSelf;
}

#[derive(Debug, BinDecode, BinEncode, PartialEq)]
pub struct PlayerLoaded {
  pub player_id: u8,
}

impl PacketPayload for PlayerLoaded {
  const PACKET_TYPE_ID: PacketTypeId = PacketTypeId::PlayerLoaded;
}

#[test]
fn test_count_down_start() {
  crate::packet::test_simple_payload_type("count_down_start.bin", &CountDownStart)
}

#[test]
fn test_count_down_end() {
  crate::packet::test_simple_payload_type("count_down_end.bin", &CountDownEnd)
}

#[test]
fn test_game_loaded_self() {
  crate::packet::test_simple_payload_type("game_loaded_self.bin", &GameLoadedSelf)
}

#[test]
fn test_player_loaded() {
  crate::packet::test_simple_payload_type("player_loaded.bin", &PlayerLoaded { player_id: 2 })
}
