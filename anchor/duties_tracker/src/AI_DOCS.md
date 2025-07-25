# Duties Tracker Component

## Overview

The Duties Tracker is a core component of the Anchor SSV (Secret Shared Validators) system that manages validator duties tracking for Ethereum validators. It monitors and tracks two main types of duties: sync committee duties and block proposal duties, along with voluntary exit tracking.

## Purpose

This component serves as the central duty management system that:
- Tracks which validators are assigned to sync committees for specific periods
- Monitors block proposal duties for validators across epochs
- Manages voluntary exit duties and scheduling
- Provides a unified interface for querying validator duties
- Ensures efficient concurrent access to duty data

## Architecture

### Core Components

1. **DutiesTracker** (`duties_tracker.rs:30-308`): Main orchestrator that polls beacon nodes for duties
2. **Duties** (`lib.rs:72-87`): Central data structure containing duty information
3. **SyncCommitteePerPeriod** (`lib.rs:28-67`): Manages sync committee duty storage
4. **VoluntaryExitTracker** (`voluntary_exit_tracker.rs:17-117`): Tracks voluntary exit duties

### Data Flow

```
Beacon Node API → DutiesTracker → Duties → {SyncCommitteePerPeriod, ProposerMap, VoluntaryExitTracker}
                                      ↓
                               DutiesProvider Interface
                                      ↓
                              Consumer Components
```

### Concurrency Design

The component is designed for high-concurrency scenarios:
- **DashMap**: Used for fine-grained locking at entry level instead of global locks
- **RwLock**: Used for proposer duties with read-heavy workloads
- **Concurrent Polling**: Separate async tasks for sync duties and proposer duties

## Key Features

### Sync Committee Duty Management
- Tracks validators assigned to sync committees by period
- Polls duties for current and next sync committee periods
- Automatic pruning of historical duties
- Memory-efficient storage (only stores validators with actual duties)

### Proposer Duty Management  
- Downloads and caches block proposal duties per epoch
- Filters duties to only include locally managed validators
- Historical duty retention with configurable lookback

### Voluntary Exit Tracking
- Schedules and tracks voluntary exit duties
- Supports duty counting for rate limiting
- Manages both own validator exits and network-wide duty tracking

### Performance Optimizations
- **Smart Polling**: Only polls when duties are unknown or outdated
- **Memory Management**: Automatic pruning of old duties
- **Concurrent Access**: Fine-grained locking minimizes contention
- **Efficient Filtering**: Only tracks relevant (local) validators

## Integration Points

### Dependencies
- `beacon_node_fallback`: Beacon node API client with fallback support
- `slot_clock`: Time synchronization with beacon chain slots
- `database::NetworkState`: Validator state management
- `ssv_types`: SSV-specific type definitions

### Interfaces
- **DutiesProvider**: Main trait for duty queries used by other components
- **TaskExecutor**: Async task management for polling loops

## Error Handling

The component implements robust error handling:
- Graceful degradation when beacon nodes are unavailable
- Retry logic with fallback beacon node support  
- Comprehensive error types for different failure scenarios
- Logging for debugging and monitoring

## Thread Safety

All data structures are designed for concurrent access:
- DashMap provides concurrent HashMap functionality
- RwLock allows multiple concurrent readers
- Arc enables safe sharing across async tasks
- No unsafe code or manual memory management