use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

// Define a trait for nodes for shared behaviour that multiple types can implement
// Does Node here correlate to "Controller" in the go code
trait Node: Clone + Send + 'static {
    //defines an associated type within the Node trait -- each implementation will specify what
    //MessageContent is for that particular implementation
    type message_content: Clone + Send + 'static;
    fn id(&self) -> usize;
    fn instance_height(&self) -> usize;
    fn process_message(&mut self, message: &Message<Self>);
}

// Changed definition of message struct to align with spec implementation but now it's busted
#[derive(Debug)]
struct Message {
    msg_type: Arc<Mutex<NodeState>>,
    instance_height: usize,
    id: usize,
    //to: usize,
    //don't know how to use this as generic
    data: String,
}

//Define struct for storing node data and initialising
#[derive(Debug, Clone)]
struct MyNode {
    id: usize,
    state: Arc<Mutex<NodeState>>,
    validators: usize,
    instance_height: usize,
    configuration: usize,
    message_content: String,
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
}

impl<N: Node> Consensus<N> {
    fn new(nodes: Vec<N>, quorum_size: usize) -> Self {
        let (sender, receiver) = mpsc::channel();

        // Start nodes' threads
        for node in nodes.clone() {
            let mut node = node;
            thread::spawn(move || {
                loop {
                    // this needs to be fixed to be reference to actual node state
                    let state = NodeState::Waiting;
                    match state {
                        NodeState::Waiting => {
                            // Transition to proposing state
                            node.process_message(&Message {
                                msg_type: Arc::new(Mutex::new(NodeState::Waiting)),
                                instance_height: node.instance_height(),
                                id: node.id(),
                                data: "Waiting".into(),
                            });
                        }
                        NodeState::Proposing => {
                            // Simulate receiving messages
                            if let Ok(message) = receiver.recv() {
                                if message.content == "Proposal" {
                                    // Transition to accepting state
                                    node.process_message(&Message {
                                        msg_type: Arc::new(Mutex::new(NodeState::Proposing)),
                                        instance_height: node.instance_height(),
                                        id: node.id(),
                                        //to: (node.id() + 1) % nodes.len(), // Example routing
                                        data: "Accept".into(),
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
            //sender,
        }
    }
}

//pub fn initialise_node(id: usize) -> {

fn main() {
    impl Node for MyNode {
        type message_content = String;
        fn instance_height(&self) -> usize {
            1
        }

        fn id(&self) -> usize {
            self.id
        }

        fn process_message(&mut self, message: &Message<Self>) {
            let mut state = self.state.lock().unwrap();
            println!("Node {} received message {:?}", self.id, message);

            // Transition states based on message content
            match message.msg_type {
                "Waiting" => *state = NodeState::Waiting,
                "Proposing" => *state = NodeState::Proposing,
                "Accepting" => *state = NodeState::Accepting,
                _ => {}
            }
        }
    }

    let nodes = vec![
        MyNode {
            id: 0,
            state: Arc::new(Mutex::new(NodeState::Waiting)),
            instance_height: 1,
            configuration: 1,
            validators: 2,
            message_content: "Waiting".to_string(),
        },
        MyNode {
            id: 1,
            state: Arc::new(Mutex::new(NodeState::Waiting)),
            instance_height: 1,
            configuration: 1,
            validators: 2,
            message_content: "Waiting".to_string(),
        },
        // Add more nodes as needed
    ];

    let Consensus = Consensus::new(nodes, 2);
}
