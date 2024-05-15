use std::{io::{self, Read}, net::{TcpListener, TcpStream}, os::fd::AsRawFd};

use runtime::*;

fn main() {
    SCHEDULER.spawn(Main::ToStartServer);
    SCHEDULER.run();

    enum Main {
        ToStartServer,
        ToAcceptConn(TcpListener),
    }

    impl Future for Main {
        type Output = ();

        fn poll(&mut self, waker: Waker) -> Option<Self::Output> {
            if let Main::ToStartServer = self {
                let server = TcpListener::bind("localhost:3000").unwrap();
                server.set_nonblocking(true).unwrap();

                REACTOR.with(|reactor: &Reactor| -> () {
                    reactor.add(server.as_raw_fd(), waker);
                });

                *self = Main::ToAcceptConn(server);
            }

            if let Main::ToAcceptConn(server) = self {
                match server.accept() {
                    Ok((connection, _)) => {
                        connection.set_nonblocking(true).unwrap();
                        SCHEDULER.spawn(ConnectionHandler {
                            connection,
                            connection_state: ConnectionState::ToStart,
                        });
                        // todo!()
                    },
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        return None;
                    },
                    Err(e) => {
                        eprintln!("{e}");
                        return Some(())
                    },
                }
            }
            todo!()
        }
    }
}

struct ConnectionHandler {
    connection: TcpStream,
    connection_state: ConnectionState,
}

impl Future for ConnectionHandler {
    type Output = ();

    fn poll(&mut self, waker: Waker) -> Option<Self::Output> {
        if let ConnectionState::ToStart = self.connection_state {
            REACTOR.with(|reactor| {
                reactor.add(self.connection.as_raw_fd(), waker);
            });

            self.connection_state = ConnectionState::Reading {
                request_data_buffer: [0u8; 1024],
                bytes_read: 0,
            };
        }

        if let ConnectionState::Reading { mut request_data_buffer, mut bytes_read } = self.connection_state {
            loop {
                match self.connection.read(&mut request_data_buffer) {
                    Ok(0) => {
                        eprintln!("connection dropped during reading request");
                        return Some(());
                    },
                    Ok(n) => {
                        bytes_read += n;
                    },
                    Err(e) => {
                        eprintln!("{e}");
                    }
                }

                // let bytes_sample = b"\r\n\r\n";

                if bytes_read >=4 && &request_data_buffer[bytes_read-4..bytes_read] == b"\r\n\r\n" /* &[u8; 4] */ {
                    break;
                }

                let hello_world_response = concat!(
                    "HTTP/1.1 200 OK\r\n",
                    "Content-Length: 12\n",
                    "Connection: close\r\n\r\n",
                    "Hello world!"
                );

                self.connection_state = ConnectionState::Writing {
                    response: hello_world_response.as_bytes(),
                    bytes_written: 0,
                }
            }
        }
        
        todo!()
    }
}

enum ConnectionState {
    ToStart,
    Reading {
        request_data_buffer: [u8; 1024],
        bytes_read: usize,
    },
    Writing {
        response: &'static [u8],
        bytes_written: usize,
    }
}

mod runtime {
    // use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use std::{collections::VecDeque, os::fd::RawFd};
    use std::collections::HashMap;

    use epoll::{ControlOptions::EPOLL_CTL_ADD, Event, Events}; 

    // use super::*;

    pub trait Future {
        type Output;

        fn poll(&mut self, waker: Waker) -> Option<Self::Output>;
    }

    pub struct Waker {
        wake_fn: Box<dyn Fn()>
    }

    impl Waker {
        fn wake(&self) {
            (self.wake_fn)();
        }
    }

    thread_local! {
        pub static REACTOR: Reactor = Reactor::new();
    }    

    pub struct Reactor {
        epoll_fd: RawFd,
        tasks: Mutex<HashMap<RawFd, Waker>>
    }

    impl Reactor {
        fn new() -> Self {
            Self {
                epoll_fd: epoll::create(false).unwrap(),
                tasks: Mutex::new(HashMap::<RawFd, Waker>::new()),
            }
        }

        pub fn add(&self, fd: RawFd, waker: Waker) {
            let event = epoll::Event::new(Events::EPOLLIN | Events::EPOLLOUT, fd as u64);
            epoll::ctl(self.epoll_fd, EPOLL_CTL_ADD, fd, event).unwrap();
            self.tasks.lock().unwrap().insert(fd, waker);
        }

        pub fn remove(&self, fd: RawFd) {
            self.tasks.lock().unwrap().remove(&fd);
        }

        pub fn wait(&self) {
            let mut events = [Event::new(Events::empty(), 0); 1024];
            let timeout = -1;
            let num_events = epoll::wait(self.epoll_fd, timeout, &mut events).unwrap();

            for event in &events[..num_events] {
                let file_descriptor_linked_to_event = event.data as RawFd;
                
                if let Some(waker) = self.tasks.lock().unwrap().get(&file_descriptor_linked_to_event) {
                    waker.wake();
                }
            }
        }
    }

    // how to tell that Mutex<T> is not Sized without using helper code?!
    type Task = Arc<Mutex<dyn Future<Output = ()> + Send>>;

    // struct CheckSizedHelperStruct<T: ?Sized>(T);
    // struct UseCheckSizedHelperStruct(CheckSizedHelperStruct<Mutex<dyn Future<Output = ()> + Send>>); 

    // static declaration requires the type of the item declared to be Sync
    pub static SCHEDULER: Scheduler = Scheduler {
        runnable_tasks: Mutex::new(VecDeque::new()),
    };

    pub struct Scheduler {
        runnable_tasks: Mutex<VecDeque<Task>>
    }    

    impl Scheduler {
        pub fn run(&self) {
            while let Some(task) = self.runnable_tasks.lock().unwrap().pop_front() {

                // let wake_fn = 
                todo!()
            }
        }

        pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
            // the one line explanation of the the fix of adding 'static to the function parameter for the
            // push_back call to come through is that the compiler needs to make sure elements in VecDeque
            // are guaranteed to be valid (otherwise think of VecDeque having elements of references that is
            // non static lifetime and may become invalid at some point). Gappy understanding?!
            self.runnable_tasks.lock().unwrap().push_back(Arc::new(Mutex::new(task)));
        }
    }

    struct Executor {
        tasks: Vec<Box<dyn Future<Output = ()>>>
    }

    impl Executor {
        fn spawn(&mut self, task: impl Future<Output = ()> + 'static) {
            self.tasks.push(Box::new(task));
        }
    }

}