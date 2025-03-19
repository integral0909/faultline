//! A service in the style of Apache Thrift's tutorial: constants of every
//! kind, typedefs, defaults, an included base service and a oneway call.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Role, validate};
use faultline_source::SourceMap;
use faultline_thrift::lower_thrift;

const SHARED: &str = r"
/**
 * Definitions shared by the tutorial's services.
 */
namespace cpp shared
namespace java shared
namespace py shared

struct SharedStruct {
  1: i32 key
  2: string value
}

service SharedService {
  SharedStruct getStruct(1: i32 key)
}
";

const TUTORIAL: &str = r#"
# A calculator service.
include "shared.thrift"

namespace cl tutorial
namespace cpp tutorial
namespace d tutorial
namespace java tutorial
namespace php tutorial
namespace perl tutorial
namespace haxe tutorial

typedef i32 MyInteger

const i32 INT32CONSTANT = 9853
const map<string,string> MAPCONSTANT = {'hello':'world', 'goodnight':'moon'}

enum Operation {
  ADD = 1,
  SUBTRACT = 2,
  MULTIPLY = 3,
  DIVIDE = 4
}

struct Work {
  1: i32 num1 = 0,
  2: i32 num2,
  3: Operation op,
  4: optional string comment,
}

exception InvalidOperation {
  1: i32 whatOp,
  2: string why
}

service Calculator extends shared.SharedService {

   void ping(),

   i32 add(1:i32 num1, 2:i32 num2),

   i32 calculate(1:i32 logid, 2:Work w) throws (1:InvalidOperation ouch),

   /**
    * This method has a oneway modifier. That means the client only makes
    * a request and does not listen for any response at all.
    */
   oneway void zip()

}
"#;

#[test]
fn the_tutorial_service_lowers() {
    let loader = MemoryLoader::new()
        .with("file:///tutorial.thrift", TUTORIAL)
        .with("file:///shared.thrift", SHARED);
    let sources = SourceMap::new();
    let lowered = lower_thrift(
        &UriRef::parse("file:///tutorial.thrift").unwrap(),
        &loader,
        &sources,
        "tutorial",
        Role::Both,
    );
    let warnings: Vec<String> = lowered
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert!(warnings.is_empty(), "{warnings:?}");
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    let mut keys: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "Calculator/add",
            "Calculator/calculate",
            "Calculator/getStruct",
            "Calculator/ping",
            "Calculator/zip",
        ]
    );
    let zip = api
        .operations
        .iter()
        .find(|o| o.key.to_string() == "Calculator/zip")
        .unwrap();
    assert!(zip.one_way);
    assert!(
        zip.meta
            .description
            .as_deref()
            .unwrap()
            .contains("oneway modifier")
    );
}
