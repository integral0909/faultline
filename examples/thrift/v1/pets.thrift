include "shared.thrift"

namespace * example.pets

typedef i64 PetId

enum Status {
  AVAILABLE = 1,
  PENDING = 2,
}

struct Pet {
  1: required PetId id,
  2: required string name,
  3: optional Status status,
  4: optional list<string> tags,
  5: optional i32 age,
}

service Pets extends shared.Health {
  Pet get(1: PetId id) throws (1: shared.NotFound missing),
  list<Pet> search(1: optional string name, 2: optional i32 limit = 20),
  oneway void refresh(1: PetId id),
}
