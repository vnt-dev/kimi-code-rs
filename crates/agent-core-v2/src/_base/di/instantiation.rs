use std::{fmt, marker::PhantomData};

use super::{errors::DiError, service_collection::ServiceValue};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ErasedServiceIdentifier {
    name: &'static str,
}

impl ErasedServiceIdentifier {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }

    pub const fn as_str(self) -> &'static str {
        self.name
    }
}

impl fmt::Display for ErasedServiceIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

pub struct ServiceIdentifier<T: ?Sized> {
    erased: ErasedServiceIdentifier,
    marker: PhantomData<fn() -> T>,
}

impl<T: ?Sized> Copy for ServiceIdentifier<T> {}

impl<T: ?Sized> Clone for ServiceIdentifier<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> ServiceIdentifier<T> {
    // Rust adaptation: parameter decorators become typed, compile-time identifiers.
    pub const fn new(name: &'static str) -> Self {
        Self {
            erased: ErasedServiceIdentifier::new(name),
            marker: PhantomData,
        }
    }

    pub const fn erase(self) -> ErasedServiceIdentifier {
        self.erased
    }

    pub const fn refine<U: ?Sized>(self) -> ServiceIdentifier<U> {
        ServiceIdentifier {
            erased: self.erased,
            marker: PhantomData,
        }
    }
}

impl<T: ?Sized> fmt::Debug for ServiceIdentifier<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ServiceIdentifier")
            .field(&self.erased.name)
            .finish()
    }
}

impl<T: ?Sized> fmt::Display for ServiceIdentifier<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.erased.fmt(formatter)
    }
}

impl<T: ?Sized, U: ?Sized> PartialEq<ServiceIdentifier<U>> for ServiceIdentifier<T> {
    fn eq(&self, other: &ServiceIdentifier<U>) -> bool {
        self.erased == other.erased
    }
}

impl<T: ?Sized> Eq for ServiceIdentifier<T> {}

pub trait ServicesAccessor: Send + Sync {
    fn get_erased(&self, id: ErasedServiceIdentifier) -> Result<ServiceValue, DiError>;
}

pub trait ServicesAccessorExt: ServicesAccessor {
    fn get<T>(&self, id: ServiceIdentifier<T>) -> Result<std::sync::Arc<T>, DiError>
    where
        T: Send + Sync + 'static,
    {
        self.get_erased(id.erase())?
            .downcast::<T>()
            .map_err(|_| DiError::TypeMismatch(id.erase()))
    }
}

impl<T: ServicesAccessor + ?Sized> ServicesAccessorExt for T {}

pub const INSTANTIATION_SERVICE_ID: ServiceIdentifier<dyn Send + Sync> =
    ServiceIdentifier::new("instantiationService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_preserve_name_identity_across_refinement() {
        let base = ServiceIdentifier::<String>::new("service");
        let refined = base.refine::<str>();
        assert_eq!(base.to_string(), "service");
        assert_eq!(base, refined);
        assert_eq!(base.erase(), ErasedServiceIdentifier::new("service"));
    }
}
