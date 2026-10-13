//! Generated protobuf types and gRPC service stubs for Zanshin.

pub mod api {
    #[allow(clippy::result_large_err, clippy::all)]
    pub mod v1 {
        tonic::include_proto!("zanshin.api.v1");
    }
}

/// Serialized `FileDescriptorSet` for gRPC server reflection.
pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("zanshin_descriptor");

pub use prost_types;
