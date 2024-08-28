use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

// Define a trait for nodes
trait Node: Clone + Send + 'static {
    type MessageContent: Clone + Send + 'static;

    fn id(&self) -> usize;
    fn process_message(&mut self, message: &Message<Self>);
}

// Define a generic Message struct
#[derive(Debug, Clone)]
struct Message<N: Node> {
    from: usize,
    to: usize,
    content: N::MessageContent,
}

// Define node states
#[derive(Debug, Clone, PartialEq)]
enum NodeState {
    Waiting,
    Proposing,
    Accepting,
}

// Define a generic Consensus struct
#[derive(Debug)]
struct Consensus<N: Node> {
    nodes: Vec<N>,
    quorum_size: usize,
    sender: Sender<Message<N>>,
}

impl<N: Node> Consensus<N> {
    fn new(nodes: Vec<N>, quorum_size: usize) -> Self {
        let (sender, receiver) = mpsc::channel();

        // Start nodes' threads
        for node in nodes.clone() {
            let receiver = receiver.clone();
            let mut node = node;
            thread::spawn(move || {
                loop {
                    // Simulate node actions
                    let state = NodeState::Waiting; // This would normally be stored in the node

                    match state {
                        NodeState::Waiting => {
                            // Transition to proposing state
                            node.process_message(&Message {
                                from: node.id(),
                                to: (node.id() + 1) % nodes.len(), // Example routing
                                content: "Proposal".into(),
                            });
                        }
                        NodeState::Proposing => {
                            // Simulate receiving messages
                            if let Ok(message) = receiver.recv() {
                                if message.content == "Proposal" {
                                    // Transition to accepting state
                                    node.process_message(&Message {
                                        from: node.id(),
                                        to: (node.id() + 1) % nodes.len(), // Example routing
                                        content: "Accept".into(),
                                    });
                                }
                            }
                        }
                        NodeState::Accepting => {
                            // Consensus reached, exit loop
                            println!("Node {} reached consensus", node.id());
                            break;
                        }
                    }
                }
            });
        }

        Consensus {
            nodes,
            quorum_size,
            sender,
        }
    }
}

fn main() {
    // Example implementation of Node and MessageContent
    #[derive(Clone)]
    struct MyNode {
        id: usize,
        state: Arc<Mutex<NodeState>>,
    }

    impl Node for MyNode {
        type MessageContent = String;

        fn id(&self) -> usize {
            self.id
        }

        fn process_message(&mut self, message: &Message<Self>) {
            let mut state = self.state.lock().unwrap();
            println!("Node {} received message {:?}", self.id, message);

            // Transition states based on message content
            match message.content.as_str() {
                "Proposal" => *state = NodeState::Proposing,
                "Accept" => *state = NodeState::Accepting,
                _ => {}
            }
        }
    }

    let nodes = vec![
        MyNode {
            id: 0,
            state: Arc::new(Mutex::new(NodeState::Waiting)),
        },
        MyNode {
            id: 1,
            state: Arc::new(Mutex::new(NodeState::Waiting)),
        },
        // Add more nodes as needed
    ];

    let consensus = Consensus::new(nodes, 2);
}
