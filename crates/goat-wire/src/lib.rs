mod codec;
mod protocol;
pub mod transport;

pub use codec::{WireConn, WireError};
pub use protocol::{
    ClientFrame, ClientId, PROTOCOL_VERSION, ResumeMode, ServerFrame, SessionId, SessionInfo,
    SessionLiveState,
};

pub type ServerConn<S> = WireConn<S, ServerFrame, ClientFrame>;
pub type ClientConn<S> = WireConn<S, ClientFrame, ServerFrame>;

#[cfg(test)]
mod tests {
    use super::*;
    use goat_protocol::{Op, TaskId};

    #[tokio::test]
    async fn client_server_roundtrip_over_duplex() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut server: ServerConn<_> = WireConn::new(a);
        let mut client: ClientConn<_> = WireConn::new(b);

        client
            .send(&ClientFrame::Hello {
                version: PROTOCOL_VERSION,
            })
            .await
            .unwrap();
        let got = server.recv().await.unwrap();
        assert_eq!(
            got,
            ClientFrame::Hello {
                version: PROTOCOL_VERSION
            }
        );

        server
            .send(&ServerFrame::Welcome {
                version: PROTOCOL_VERSION,
                client_id: ClientId(7),
            })
            .await
            .unwrap();
        let got = client.recv().await.unwrap();
        assert_eq!(
            got,
            ServerFrame::Welcome {
                version: PROTOCOL_VERSION,
                client_id: ClientId(7)
            }
        );
    }

    #[tokio::test]
    async fn submit_op_frame_roundtrips() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut server: ServerConn<_> = WireConn::new(a);
        let mut client: ClientConn<_> = WireConn::new(b);
        let frame = ClientFrame::Submit {
            session: SessionId(1),
            correlation: 42,
            op: Op::SubmitMessage {
                id: TaskId(0),
                text: "hi".to_owned(),
            },
        };
        client.send(&frame).await.unwrap();
        assert_eq!(server.recv().await.unwrap(), frame);
    }
}
