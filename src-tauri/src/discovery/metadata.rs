//! Decode metadata only. EOF in a bounded prefix preserves decoded metadata through
//! capture seeds; unknown fields use IgnoredAny, so message content is never copied.
use serde::{
    de::{DeserializeSeed, MapAccess, Visitor},
    Deserialize, Deserializer,
};
struct Capture<'a, T>(&'a mut T);
macro_rules! metadata {
    ($name:ident { $($field:ident : $ty:ty = $wire:literal),* $(,)? } { $($nested:ident : $child:ty = $nested_wire:literal),* $(,)? }) => {
        #[derive(Default,Deserialize)] #[serde(default)] pub(super) struct $name {
            $(#[serde(rename=$wire)] pub $field:$ty,)*
            $(#[serde(rename=$nested_wire)] pub $nested:Option<$child>,)*
        }
        impl<'de> Visitor<'de> for Capture<'_,$name> {
            type Value=();
            fn expecting(&self,f:&mut std::fmt::Formatter)->std::fmt::Result{f.write_str("metadata object")}
            fn visit_map<M:MapAccess<'de>>(self,mut map:M)->Result<(),M::Error>{
                while let Some(key)=map.next_key::<String>()? {
                    match key.as_str() {
                        $($wire=>{self.0.$field=map.next_value()?;},)*
                        $($nested_wire=>{map.next_value_seed(Capture(self.0.$nested.get_or_insert_with(Default::default)))?;},)*
                        _=>{map.next_value::<serde::de::IgnoredAny>()?;}
                    }
                }
                Ok(())
            }
        }
        impl<'de> DeserializeSeed<'de> for Capture<'_,$name> {
            type Value=();
            fn deserialize<D:Deserializer<'de>>(self,d:D)->Result<(),D::Error>{d.deserialize_map(self)}
        }
    }
}
metadata!(Record {
    kind:String="type",v:Option<u32>="v",title:Option<String>="title",pad:Option<String>="pad",updated_at:Option<String>="updatedAt",id:Option<String>="id",timestamp:Option<String>="timestamp",cwd:Option<String>="cwd",model:Option<String>="model",provider:Option<String>="provider",model_id:Option<String>="modelId",role:Option<String>="role",name:Option<String>="name",session_id:Option<String>="sessionId",custom_title:Option<String>="customTitle",ai_title:Option<String>="aiTitle",session_kind:Option<String>="sessionKind",is_sidechain:bool="isSidechain"
} {message:Message="message",payload:Payload="payload"});
metadata!(Message {role:Option<String>="role",model:Option<String>="model",provider:Option<String>="provider"} {});
metadata!(Payload {id:Option<String>="id",session_id:Option<String>="session_id",cwd:Option<String>="cwd",timestamp:Option<String>="timestamp",model:Option<String>="model",model_provider:Option<String>="model_provider",thread_source:Option<String>="thread_source",source:Option<ThreadSource>="source"} {});
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum ThreadSource {
    Name(String),
    Other(serde::de::IgnoredAny),
}
pub(super) fn decode(bytes: &[u8]) -> Option<Record> {
    match serde_json::from_slice(bytes) {
        Ok(r) => Some(r),
        Err(e) if e.is_eof() => {
            let mut r = Record::default();
            let _ = Capture(&mut r).deserialize(&mut serde_json::Deserializer::from_slice(bytes));
            (!r.kind.is_empty()).then_some(r)
        }
        Err(_) => None,
    }
}
#[cfg(test)]
mod tests;
