use crate::{Result, failure};
use kinakaze_v2_host_win::PipeConnection;
use kinakaze_v2_protocol::{
    ClientRole, Hello, PROTOCOL_VERSION, Reply, Request, WireRequest, WireResponse, read_frame,
    write_frame,
};

pub struct Controller {
    pipe: PipeConnection,
    next_id: u64,
}

impl Controller {
    pub fn connect(endpoint: &str, token: String) -> Result<Self> {
        let mut client = Self {
            pipe: PipeConnection::connect(endpoint)?,
            next_id: 1,
        };
        match client.call(Request::Hello(Hello {
            version: PROTOCOL_VERSION,
            token,
            role: ClientRole::Controller,
            adoption_ticket: None,
        }))? {
            Reply::Hello { process: None, .. } => Ok(client),
            _ => Err(failure("controller received invalid Hello response")),
        }
    }

    pub fn call(&mut self, request: Request) -> Result<Reply> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| failure("RPC id exhausted"))?;
        write_frame(&mut self.pipe, &WireRequest { id, request })?;
        let response: WireResponse = read_frame(&mut self.pipe)?;
        if response.id != id {
            return Err(failure("controller RPC response id mismatch"));
        }
        Ok(response.result?)
    }
}
