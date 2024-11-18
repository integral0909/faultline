namespace * example.shared

/** Raised when a request names something that does not exist. */
exception NotFound {
  1: required string message
}

service Health {
  string version()
}
