use pepino_core::BoardId;
use sqlx::{
    Encode, Sqlite, Type,
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentsBuffer, SqliteTypeInfo},
};

pub(crate) struct BoardKey(pub BoardId);

impl Type<Sqlite> for BoardKey {
    fn type_info() -> SqliteTypeInfo {
        <[u8; 32] as Type<Sqlite>>::type_info()
    }
}

impl Encode<'_, Sqlite> for BoardKey {
    fn encode_by_ref(&self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        self.0.as_bytes().encode_by_ref(args)
    }
}
