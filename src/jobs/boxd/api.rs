//! The messages of boxd's gRPC API (`boxd.api.v1.BoxdApi`) that orx uses, with the tags of
//! <https://github.com/boxd-sh/sdk/blob/main/proto/api.proto>. Decoding skips the fields left out.

use std::collections::HashMap;

/// `StartVm`, `WakeVm`, `ResumeVm`, `GetVm` and `DestroyVm` take only the machine's id.
#[derive(Clone, PartialEq, prost::Message)]
pub struct VmId {
    #[prost(string, tag = "1")]
    pub vm_id: String,
}

/// Requests without fields, and the responses orx doesn't read.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct Empty {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CreateVmRequest {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, optional, tag = "8")]
    pub config: Option<VmConfig>,
    #[prost(bool, tag = "12")]
    pub isolated: bool,
    #[prost(map = "string, string", tag = "14")]
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct VmConfig {
    #[prost(uint32, tag = "1")]
    pub vcpu: u32,
    #[prost(uint64, tag = "2")]
    pub memory_bytes: u64,
    #[prost(message, optional, tag = "4")]
    pub srf: Option<SrfConfig>,
}

#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct SrfConfig {
    #[prost(uint32, optional, tag = "1")]
    pub auto_suspend_timeout_secs: Option<u32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CreateVmResponse {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(message, optional, tag = "9")]
    pub machine: Option<Vm>,
}

/// `GetVmResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Vm {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(string, tag = "5")]
    pub status: String,
    #[prost(uint32, tag = "15")]
    pub vcpu: u32,
    #[prost(map = "string, string", tag = "34")]
    pub labels: HashMap<String, String>,
    #[prost(string, tag = "42")]
    pub region: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ListVmsRequest {
    #[prost(string, repeated, tag = "3")]
    pub label_selector: Vec<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ListVmsResponse {
    #[prost(message, repeated, tag = "1")]
    pub vms: Vec<Vm>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SetAutoHibernateTimeoutRequest {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(uint32, tag = "2")]
    pub timeout_secs: u32,
}

/// `GetOrgMachineDefaultsResponse`; the request names no org, so it is the API key's own.
#[derive(Clone, PartialEq, prost::Message)]
pub struct OrgDefaults {
    #[prost(uint32, tag = "2")]
    pub vcpu: u32,
    #[prost(uint32, tag = "4")]
    pub ceiling_vcpu: u32,
    #[prost(string, tag = "7")]
    pub region: String,
}

/// Both directions of `Exec`: the first request chunk names the machine and the command; the
/// last response chunk has no data and carries the exit code.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ExecChunk {
    #[prost(bytes = "vec", tag = "1")]
    pub data: Vec<u8>,
    #[prost(string, tag = "4")]
    pub command: String,
    #[prost(string, tag = "5")]
    pub vm_id: String,
    #[prost(int32, tag = "6")]
    pub exit_code: i32,
    #[prost(bool, tag = "10")]
    pub is_stderr: bool,
}

/// `UploadFileStream`: the first chunk names the machine, the path and the size.
#[derive(Clone, PartialEq, prost::Message)]
pub struct UploadFileChunk {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(string, tag = "2")]
    pub path: String,
    #[prost(uint64, tag = "3")]
    pub total_size: u64,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,
}
