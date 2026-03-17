## Architecture and design

Anchor is a modular Rust workspace with service-oriented architecture. Preserve crate boundaries and typed interfaces.

### Crate structure
- Each major component is its own crate with a minimal, well-documented public API
- Crates should only depend on what they need
- Each crate has clear responsibility and ownership

### Dependency flow
- Common types and utilities live in `common/` subdirectories
- Higher-level services depend on lower-level ones, not vice versa
- Config flows down from client to components; events flow up to coordinators

### Inter-component communication
- Components communicate via typed message channels
- System-wide events use the EventBus pattern
- Components interact through trait interfaces, not concrete implementations
- Errors are properly typed and propagated up the stack

### Design principles
1. **Question intermediaries**: If data flows A -> B with no transformation, question why an intermediate layer exists. Each layer must provide clear value (logging, transformation, validation, etc.)
2. **Separation through interfaces, not layers**: Clean boundaries come from well-defined APIs, not intermediary components
3. **Simplification is always valid**: Refactoring working code for simplicity is encouraged. Fewer lines and fewer components often indicates better design
4. **Challenge complexity**: Every abstraction must justify its existence. "We might need it later" is not sufficient. Complexity must solve specific, current problems
5. **Prefer direct communication**: Unless an intermediary adds clear value (validation, transformation, isolation, or observability), components should communicate directly
