use crate::ecs::{Component, GameObject};
use anyhow::{Result, anyhow};
use std::cell::RefCell;
use std::rc::Weak;

#[derive(Default)]
pub(crate) struct WeakComponentList {
    components: RefCell<Vec<Weak<RefCell<dyn Component>>>>,
}

impl WeakComponentList {
    pub(crate) fn push(&self, component: Weak<RefCell<dyn Component>>) {
        self.components.borrow_mut().push(component);
    }

    pub(crate) fn live_count(&self) -> usize {
        self.components
            .borrow()
            .iter()
            .filter(|component| component.upgrade().is_some())
            .count()
    }

    pub(crate) fn iterate_and_remove_invalid(
        &self,
        mut action: impl FnMut(&mut dyn Component, &GameObject) -> Result<()>,
    ) -> Result<()> {
        let components = self.components.borrow().clone();
        let result = components.iter().try_for_each(|weak| {
            let Some(component) = weak.upgrade() else {
                return Ok(());
            };
            let mut component = component.borrow_mut();
            let owner = component
                .owner
                .upgrade()
                .ok_or_else(|| anyhow!("component owner no longer exists"))?;
            action(&mut *component, &owner.borrow())
        });
        self.components
            .borrow_mut()
            .retain(|component| component.upgrade().is_some());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::component::{ComponentInstance, component_ref};
    use std::any::Any;
    use std::rc::Rc;

    #[crate::ecs::component]
    #[derive(crate::ecs::Component)]
    struct Probe {}

    impl Component for Probe {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    fn prunes_expired_entries_even_after_a_callback_error() {
        let owner = GameObject::empty("Owner");
        let make_component = || {
            let component = component_ref(Box::new(Probe {
                instance: ComponentInstance::default(),
            }));
            component.borrow_mut().owner = Rc::downgrade(&owner);
            component
        };
        let first = make_component();
        let dead = make_component();
        let last = make_component();
        let list = WeakComponentList::default();
        list.push(Rc::downgrade(&first));
        list.push(Rc::downgrade(&dead));
        list.push(Rc::downgrade(&last));
        drop(dead);
        let mut calls = 0;
        let error = list
            .iterate_and_remove_invalid(|_, _| {
                calls += 1;
                Err(anyhow!("callback failed"))
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "callback failed");
        assert_eq!(calls, 1);
        assert_eq!(list.components.borrow().len(), 2);
        list.iterate_and_remove_invalid(|_, _| {
            assert_eq!(list.live_count(), 2);
            calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, 3);
    }
}
