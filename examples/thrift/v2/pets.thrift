include "shared.thrift"

namespace * example.pets

typedef i64 PetId

enum Status {
  AVAILABLE = 1,
  PENDING = 2,
  ADOPTED = 3,
}

exception Throttled {
  1: required i32 retry_after_seconds
}

struct Pet {
  1: required PetId id,
  2: required string name,
  3: optional Status status,
  4: optional set<string> tags,
  5: optional i64 age,
  6: optional string owner,
}

service Pets extends shared.Health {
  Pet get(1: PetId id) throws (1: shared.NotFound missing, 2: Throttled slow),
  list<Pet> search(1: optional string name, 3: optional i32 limit = 20),
  void refresh(1: PetId id),
}
