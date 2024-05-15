use std::io::{self, prelude::*};
use std::net::{TcpListener, ToSocketAddrs};
use std::os::fd::{RawFd, AsRawFd};
use std::collections::HashMap;
use epoll::{self, Event, Events, ControlOptions::EPOLL_CTL_ADD};

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

fn run_epolled_single_thread_server<T: ToSocketAddrs> (addr: T) {
    let listener: TcpListener = TcpListener::bind(addr).expect("failed to bind to socket address to listen for connections");
    listener.set_nonblocking(true).expect("failed to set non blocking connection listener");

    // create an epoll instance, able to be linked to a set of file descriptors, as a file descriptor
    let epoll_fd: RawFd = epoll::create(false).unwrap();    

    // an event in a two-part pair, one part for the interest flag to register the type of I/O
    // events of interest, i.e. Read event in this case, another part for the data field to identify
    // the resource for which the monitor of events of interest take place, i.e. the tcp listener here
    let event = Event::new(Events::EPOLLIN, listener.as_raw_fd() as u64);
    // register the tcp listener and the associated interested events to epoll
    epoll::ctl(epoll_fd, EPOLL_CTL_ADD, listener.as_raw_fd(), event).unwrap();

    let mut connections = HashMap::new();

    loop {
        let mut events: [Event; 1024] = [Event::new(Events::empty(), 0); 1024];
        let timeout = -1;
        
        // in the first loop, epoll wait blocks until some event is delivered from the tcp listener file
        // description, which is the only element in the file description set of the epoll at the point.
        // Since Read interest had been monitored on this resource, the delivery of the first event signals
        // that a new connection is ready to be accepted
        let num_events = epoll::wait(epoll_fd, timeout, &mut events).unwrap();

        let mut completed = Vec::new();

        // process_conn encompass of executing either one of two types of tasks, i.e.
        // accept new connections, or process accepted connection to futher state of response,
        // depending on the readiness information given by the event
        'process_conn: for event in &events[..num_events] {

            // in case the event is linked to the file descriptor of tcp listener,
            // it is clear that new connection is ready to be accepted
            let fd = event.data as i32;
            if fd == listener.as_raw_fd() {
                // try accepting a connection
                match listener.accept() {
                    Ok((connection, _)) => {
                        connection.set_nonblocking(true).unwrap();

                        let fd = connection.as_raw_fd();

                        // register the connection with epoll, so that when read or write operation is ready to
                        // to take place in the connection, that is notified via event
                        let event = Event::new(Events::EPOLLIN | Events::EPOLLOUT, fd as _);
                        epoll::ctl(epoll_fd, EPOLL_CTL_ADD, fd, event).unwrap();

                        let state = ConnectionState::Read {
                            request: [0u8; 1024],
                            read: 0,
                        };
                        connections.insert(fd, (connection, state));
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(err) => panic!("failed to accept: {err}"),
                }
                continue 'process_conn;
            }

            // otherwise, the event is linked to an accepted connection that is ready to perform either
            // input or output or. Hence the file descriptor identifying the connection is obtained from 
            // the event, to retrieve the respective stored connection and process it depending on its
            // current state accordingly
            let (connection, state) = connections.get_mut(&fd).unwrap();

            if let ConnectionState::Read { request, read } = state {
                loop {
                    match connection.read(&mut *request) {
                        Ok(0) => {
                            println!("client disconnected unexpectedly");
                            completed.push(fd);
                            continue 'process_conn;
                        }
                        Ok(n) => {
                            // keep track of how many bytes we've read
                            *read += n;
                        }
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                            // not ready yet, move on to the next connection processing task
                            continue 'process_conn;
                        }
                        Err(err) => panic!("{err}"),
                    }

                    if request.get(*read - 4..*read) == Some(b"\r\n\r\n") {
                        break;
                    }
                }

                // this point must be reached from breaking the loop of processing a connection
                // started from the Read state, meaning we're done reading the whole request data
                let request = String::from_utf8_lossy(&request[..*read]);
                println!("{request}");
                // move into the write state
                let response = concat!(
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
                            // client disconnected, mark this connection as complete
                            completed.push(fd);
                            continue 'process_conn;
                        }
                        Ok(n) => {
                            *written += n;
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            continue 'process_conn;
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
                    Ok(_) => {
                        completed.push(fd);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        continue 'process_conn;
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        }

        for fd in completed {
            let (connection, _state) = connections.remove(&fd).unwrap();
            // unregister from epoll
            drop(connection);
        }           
    }
}

fn main() {
    run_epolled_single_thread_server("localhost:3000");
}