pub struct Event;

pub trait EventHandler: Send + 'static {
    fn handle_event(&mut self, ev: Event);
}

impl<T> EventHandler for T
where
    T: FnMut(Event) + Send + 'static,
{
    fn handle_event(&mut self, ev: Event) {
        self(ev)
    }
}
