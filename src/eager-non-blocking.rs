use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::io;
use std::io::prelude::*;

enum ConnectionState {
    Read {
        request: [u8; 1024],
        read: usize,
    },
    Write {
        response: &'static [u8],
        written: usize,
    },
    Flush,
}

fn run_eager_polled_single_thread_server<T: ToSocketAddrs>(addr: T) {
    let listener: TcpListener = TcpListener::bind(addr).expect("failed to bind to port");
    
    // later calls to TcpListener's accept() method become non-blocking
    listener.set_nonblocking(true).unwrap();

    // storage of all accepcted connections
    let mut connections: Vec<(TcpStream, ConnectionState)> = Vec::<(TcpStream, ConnectionState)>::new();

    loop {
        match listener.accept() {
            // in case of successfully accepting a new connection in non-blockingly, add the connection
            // TcpStream with its initiated Read state to the all accepted conns storage
            Ok((connection, _socket_addr)) => {
                
                // later calls to TcpStream's read(), write(), etc methods become non-blocking
                connection.set_nonblocking(true).unwrap();
                
                let state: ConnectionState = ConnectionState::Read {
                    request: [0u8; 1024],
                    read: 0,
                };

                connections.push((connection, state));
            },
            // in case of accepting a new connection causing a block, move on (to connection processing effort) 
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {},
            Err(err) => panic!("failed to accept tcp connection: {err}"),
        }

        // loop-local storage for all accepted connections to be dropped (in the end of the current loop) due to
        // being determined to not possible to make further processing progress
        let mut conns_to_drop_indecies = Vec::<usize>::new();

        'process_conns: for (conn_idx, (connection, state)) in connections.iter_mut().enumerate() {
            if let ConnectionState::Read { request, read } = state {
                loop {
                    match connection.read(&mut request[*read..]) {
                        // reading 0 bytes non-blockingly implies the connection is disconnected & will not make more progress
                        Ok(0) => {
                            conns_to_drop_indecies.push(conn_idx);
                            continue 'process_conns;
                        }
                        Ok(n) => {
                            *read += n;
                        }
                        // in case the attempt to read connection blocks, move on to the next accepted connection for processing
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                            continue 'process_conns;
                        }
                        Err(err) => panic!("{err}"),
                    }
                    if request.get(*read - 4..*read) == Some(b"\r\n\r\n") {
                        break;
                    }
                }
                
                let response: &'static str = concat!(
                    "HTTP/1.1 200 OK\r\n",
                    "Content-Length: 12\n",
                    "Connection: close\r\n\r\n",
                    "Hello world!"
                );
                *state = ConnectionState::Write {
                    response: response.as_bytes(),
                    written: 0,
                };
            }

            if let ConnectionState::Write { response, written } = state {
                loop {
                    match connection.write(&response[*written..]) {
                        Ok(0) => {
                            conns_to_drop_indecies.push(conn_idx);
                            continue 'process_conns;
                        }
                        Ok(n) => {
                            *written += n;
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            continue 'process_conns;
                        }
                        Err(e) => panic!("{e}"),
                    }
                    if *written == response.len() {
                        break;
                    }
                }
                *state = ConnectionState::Flush;
            }

            if let ConnectionState::Flush = state {
                match connection.flush() {
                    // active connection processed end-to-end so that it can be dropped
                    Ok(_) => {
                        conns_to_drop_indecies.push(conn_idx);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        continue 'process_conns;
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        }
        
        // dropping those conns from the conns storage found to be no longer active during the 
        // the iteration of checking all accepted conns and attempting to process each of them
        for i in conns_to_drop_indecies.into_iter().rev() {
            connections.remove(i);
        }
    }
}
fn main() {
    run_eager_polled_single_thread_server("localhost:3000");
}