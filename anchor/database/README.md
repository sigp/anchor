# Anchor Database

The Anchor Database provides a robust persistent and in-memory caching layer for the Anchor project, specifically designed to handle SSV Network data efficiently. This crate manages both persistent storage of blockchain event data and high-performance in-memory access patterns.

## Table of Contents

1. [Overview](#overview)
2. [Core Features](#core)
3. [Architecture](#Architecture)
4. [Data Models](#Data)

## Overview

The Anchor Database serves as the backbone for storing and accessing SSV Network event data. When an Anchor node starts up, it needs to process and store blockchain event logs to maintain state.

## Core Features
* **Persistent Storage**: SQLite-based store with automatic schema management
* **In-Memory Caching**: Efficient caching of frequently accessed data
* **Multi-Index Access**: Flexible data access patters through multiple different keys
* **Automatic State Recovery**: Rebuilds in-memory state from persistent storage on startup.
* **Thread Safety**: Concurrent access support through `DashMap` implementations


## Architecture
The database architecture consists of a two key layers

### Storage Layer

At the foundation lies a SQLite database that provides persistent storage. This layer encompasses
* **Database Connection Management**: A connection pool that maintains and reuses SQLite connections efficiently, preventing resource exhaustion while ensuring consistent access
* **Schema and Transaction Management**: Automatic table creation and transaction support for data integrity


### Cache Layer
The in-memory cache layer combines high-performance caching with sophisticated indexing through a unified system. Is is broken up into Single-State and Multi-State.

* **Single State**: Single state handles straightforward, one-to-one relationships where data only needs one access pattern. This is ideal for data that is frequently access but has simple relationships.
* **Multi State**: Multi State handles complex relationships where the same data needs to be accessed through different keys. This is implemented through a series of `MultiIndexMap`s, each supporting three different access patterns for the same data. The type system enforces correct usage through the `UniqueTag` and `NonUniqueTag` markers, preventing incorrect access patterns at compile time. Each `MultiIndexMap` in the Multi State provides three ways to access its data:
    1) A primary key that uniquely identifies each piece of data
    2) A secondary key that can either uniquely identify data or map to multiple items
    3) A tertiary key that can also be unique or map to multiple items

## Data Models
The database handles several core data types

**Operator**
* Represents a network operator
* Identified by `OperatorId`
* Contains RSA public key and owner address

**Cluster**
* Represents a group of Operators managing validators
* Contains cluster membership information
* Tracks operational status and fault counts

**Validator**
* Contains validator metadata
* Links to cluster membership
* Stores configuration data

**Share**
* Represents cryptographic shares for validators
* Links operators to validators
* Contains encrypted key data
