# Changelog

## [0.7.1](https://github.com/Stephan-MC/ruvel/compare/ruvel-v0.7.0...ruvel-v0.7.1) (2026-11-13)


### Bug Fixes

* **routing:** remove api_resource numeric-id constraint workaround ([77f1453](https://github.com/Stephan-MC/ruvel/commit/77f1453957fa0f3b4675769222a2c0fa8d66fd3f))


### Performance Improvements

* **routing,http,pipeline:** optimize router dispatch and middleware hot path ([558217d](https://github.com/Stephan-MC/ruvel/commit/558217de39f4197732d483106246780a5792387c))

## [0.7.0](https://github.com/Stephan-MC/ruvel/compare/ruvel-v0.6.0...ruvel-v0.7.0) (2026-11-12)


### Features

* **routing:** Added a resource Registrar for registring Resource routes ([c751b46](https://github.com/Stephan-MC/ruvel/commit/c751b46fbfaa45e7fa8a4b2e8256c08f45d369f5))

## [0.6.0](https://github.com/Stephan-MC/ruvel/compare/ruvel-v0.5.0...ruvel-v0.6.0) (2026-11-12)


### Features

* Added RoutingServiceProvider ([7103651](https://github.com/Stephan-MC/ruvel/commit/7103651281f4670f2c05dcb8b8f0227a06226503))
* **configuration:** implement Routing builder and update with_routing callback ([2e68bda](https://github.com/Stephan-MC/ruvel/commit/2e68bda9a24be4aa9165e9fb9972af20a650fc46))


### Bug Fixes

* Adjusted single registration for RoutingServiceProvider ([4fc5435](https://github.com/Stephan-MC/ruvel/commit/4fc54352333d34e7ceeb0d34aefaaaf9efec9a01))
* Fix http::Kernel container binding to used already bound router instance from container ([eb5f1dd](https://github.com/Stephan-MC/ruvel/commit/eb5f1dd6574bab8f412155eb547079040baa13e2))

## [0.5.0](https://github.com/Stephan-MC/ruvel/compare/ruvel-v0.4.0...ruvel-v0.5.0) (2026-11-11)


### Features

* Added support for dynamic routing ([5d8e3ce](https://github.com/Stephan-MC/ruvel/commit/5d8e3ce2c657e7597ab85a020d235a9d8b95d673))
* **database:** implement Eloquent models and base providers ([c7b71e9](https://github.com/Stephan-MC/ruvel/commit/c7b71e9a78a6a7b7fb04588d468835d43081bd16))
* **database:** implement Eloquent-style models ([d19f3ff](https://github.com/Stephan-MC/ruvel/commit/d19f3ff7e9f203c9bcf5cddcf0cb0a384be9a64a))
* implement crate::pipeline module for onion layer ([d29a253](https://github.com/Stephan-MC/ruvel/commit/d29a2538e0da7c59fb0423ad9d93bdea80dd7135))
* implement pipeline onion layer with Pipeable contract ([7672e2d](https://github.com/Stephan-MC/ruvel/commit/7672e2dc008706a2081811f65b54c18742f5db7a))
* Implemented Middleware ([286c17a](https://github.com/Stephan-MC/ruvel/commit/286c17a011be133c1444428cd098ead2c049ad8a))
* optimize onion pipeline and decouple Next from pipes ([aa55c7e](https://github.com/Stephan-MC/ruvel/commit/aa55c7e8a46b7d4329c5ca93a65eecac5b6fd443))
* **pipeline:** add prepend, append, is_empty, and Clone to Pipeline ([a2c5ce0](https://github.com/Stephan-MC/ruvel/commit/a2c5ce0c6414cfe16bfea8917af78c887f5910c1))
* **routing:** introduce RouteRegistrar trait and unify PendingRouteRegistrar ([5280fcf](https://github.com/Stephan-MC/ruvel/commit/5280fcf1a4615a9232ace7e1426cd670c173455d))
* split pipeline and pipe implementations into separate files ([1e96e22](https://github.com/Stephan-MC/ruvel/commit/1e96e22f8548b3ad52d42ebd116906fc97c3d164))
* Updated memory allocations during keep-alive to tripple throughput ([3be46bb](https://github.com/Stephan-MC/ruvel/commit/3be46bb61adc6db3357a050d8b951211a46a0c55))
* Updated memory allocations during keep-alive to tripple throughput ([7cab6b6](https://github.com/Stephan-MC/ruvel/commit/7cab6b644c3e2871b67606d6e97d5b393f79f9e9))


### Bug Fixes

* Added release-please to enable automatic releases ([f860c0d](https://github.com/Stephan-MC/ruvel/commit/f860c0df63d1f22fc900c3001d21dfc31d98c0b7))
* Added release-please-token to ci ([7320ab9](https://github.com/Stephan-MC/ruvel/commit/7320ab9ff8437b34c9acc4b98f98cab158df9962))
* Address controller async-based warnings on terminal when running server ([12eb15c](https://github.com/Stephan-MC/ruvel/commit/12eb15cc719ab0dd1a13a1497f0a49c4d0fba9ef))
* Address issue with kernel not handling requests ([abfecd2](https://github.com/Stephan-MC/ruvel/commit/abfecd2eeea39b0641e3594d51cfc4c2690308c1))
* Bumped to v0.1.4 ([4bf60e4](https://github.com/Stephan-MC/ruvel/commit/4bf60e445d8a16c78d29031bb62ab51d7d359c68))
* Bumped to v0.1.4 ([ac4adab](https://github.com/Stephan-MC/ruvel/commit/ac4adab00677c115871fdb6240c77e0fa86e521f))
* Bumped to verison 0.1.2 ([82f83e4](https://github.com/Stephan-MC/ruvel/commit/82f83e4d074bfc2499ead24b69da217aaef60a00))
* Bumped to verison 0.1.2 ([354422b](https://github.com/Stephan-MC/ruvel/commit/354422b3f684f4094a33a73246aae765ba517b2b))
* Little formatting tweaks. ([29e203f](https://github.com/Stephan-MC/ruvel/commit/29e203f6a839a54f874a31bfe30636479e169a9c))
* Moved middleware into a separate path ([7836416](https://github.com/Stephan-MC/ruvel/commit/7836416e3e6381dcde49766fd04f6efb440fec66))
* Moved middleware into a separate path ([7e72286](https://github.com/Stephan-MC/ruvel/commit/7e7228639de2d6960e5e760796384047db844405))
* Removed cargo-workspace plugin from release-please-config ([5417990](https://github.com/Stephan-MC/ruvel/commit/54179900d28745141f86263041d08ab555d028ef))
* Removed duplicate imports ([1f0f58f](https://github.com/Stephan-MC/ruvel/commit/1f0f58f05f231cccdaa743f291a8c5ee7e75e93f))
* Removed unnecessary lines from release-please-configs ([2a07bf6](https://github.com/Stephan-MC/ruvel/commit/2a07bf653630cba7674d9ea54d6f82095fb16e9e))
* Renamed package release key ([432480e](https://github.com/Stephan-MC/ruvel/commit/432480ec6d0f2c7a45aa3e40281e20b9e53daf52))
* Request::Method now implements the Display trait ([c311a6b](https://github.com/Stephan-MC/ruvel/commit/c311a6b8a8077921cc6e271faabe79d807c8f4e9))
* resolve kernel merge conflict and http re-exports ([9578c5d](https://github.com/Stephan-MC/ruvel/commit/9578c5dd90a4ee589b65282a427cd3e4f2fffa84))
* **routing:** normalize trailing slashes on route registration and dispatch ([741bfd7](https://github.com/Stephan-MC/ruvel/commit/741bfd7a8aefeb79f51b199f827852b3209c0fb1))
* spawn tokio process in keep-alive mode ([f7d1e09](https://github.com/Stephan-MC/ruvel/commit/f7d1e09b86cd49c8c1effbbed9912d0f54a71733))
* Updated the request header to support specific types of data ([b5b586b](https://github.com/Stephan-MC/ruvel/commit/b5b586ba35d9594244dba8686b22452e36918d8c))


### Performance Improvements

* optimize throughput, memory allocation, and pipelining ([d483e7a](https://github.com/Stephan-MC/ruvel/commit/d483e7a1d89933c17e2f73ea89734de2aa42d2f8))

## [0.4.0](https://github.com/Stephan-MC/ruvel/compare/ruvel-app-v0.3.0...ruvel-app-v0.4.0) (2026-11-09)


### Features

* implement crate::pipeline module for onion layer ([d29a253](https://github.com/Stephan-MC/ruvel/commit/d29a2538e0da7c59fb0423ad9d93bdea80dd7135))
* implement pipeline onion layer with Pipeable contract ([7672e2d](https://github.com/Stephan-MC/ruvel/commit/7672e2dc008706a2081811f65b54c18742f5db7a))
* optimize onion pipeline and decouple Next from pipes ([aa55c7e](https://github.com/Stephan-MC/ruvel/commit/aa55c7e8a46b7d4329c5ca93a65eecac5b6fd443))
* split pipeline and pipe implementations into separate files ([1e96e22](https://github.com/Stephan-MC/ruvel/commit/1e96e22f8548b3ad52d42ebd116906fc97c3d164))


### Bug Fixes

* Little formatting tweaks. ([29e203f](https://github.com/Stephan-MC/ruvel/commit/29e203f6a839a54f874a31bfe30636479e169a9c))
* Moved middleware into a separate path ([7836416](https://github.com/Stephan-MC/ruvel/commit/7836416e3e6381dcde49766fd04f6efb440fec66))
* Moved middleware into a separate path ([7e72286](https://github.com/Stephan-MC/ruvel/commit/7e7228639de2d6960e5e760796384047db844405))
* Removed duplicate imports ([1f0f58f](https://github.com/Stephan-MC/ruvel/commit/1f0f58f05f231cccdaa743f291a8c5ee7e75e93f))
* resolve kernel merge conflict and http re-exports ([9578c5d](https://github.com/Stephan-MC/ruvel/commit/9578c5dd90a4ee589b65282a427cd3e4f2fffa84))

## [0.3.0](https://github.com/Stephan-MC/ruvel/compare/ruvel-app-v0.2.0...ruvel-app-v0.3.0) (2026-11-07)


### Features

* Added support for dynamic routing ([5d8e3ce](https://github.com/Stephan-MC/ruvel/commit/5d8e3ce2c657e7597ab85a020d235a9d8b95d673))
* Implemented Middleware ([286c17a](https://github.com/Stephan-MC/ruvel/commit/286c17a011be133c1444428cd098ead2c049ad8a))
* Updated memory allocations during keep-alive to tripple throughput ([3be46bb](https://github.com/Stephan-MC/ruvel/commit/3be46bb61adc6db3357a050d8b951211a46a0c55))
* Updated memory allocations during keep-alive to tripple throughput ([7cab6b6](https://github.com/Stephan-MC/ruvel/commit/7cab6b644c3e2871b67606d6e97d5b393f79f9e9))


### Bug Fixes

* Added release-please to enable automatic releases ([f860c0d](https://github.com/Stephan-MC/ruvel/commit/f860c0df63d1f22fc900c3001d21dfc31d98c0b7))
* Added release-please-token to ci ([7320ab9](https://github.com/Stephan-MC/ruvel/commit/7320ab9ff8437b34c9acc4b98f98cab158df9962))
* Address controller async-based warnings on terminal when running server ([12eb15c](https://github.com/Stephan-MC/ruvel/commit/12eb15cc719ab0dd1a13a1497f0a49c4d0fba9ef))
* Address issue with kernel not handling requests ([abfecd2](https://github.com/Stephan-MC/ruvel/commit/abfecd2eeea39b0641e3594d51cfc4c2690308c1))
* Bumped to v0.1.4 ([4bf60e4](https://github.com/Stephan-MC/ruvel/commit/4bf60e445d8a16c78d29031bb62ab51d7d359c68))
* Bumped to v0.1.4 ([ac4adab](https://github.com/Stephan-MC/ruvel/commit/ac4adab00677c115871fdb6240c77e0fa86e521f))
* Bumped to verison 0.1.2 ([82f83e4](https://github.com/Stephan-MC/ruvel/commit/82f83e4d074bfc2499ead24b69da217aaef60a00))
* Bumped to verison 0.1.2 ([354422b](https://github.com/Stephan-MC/ruvel/commit/354422b3f684f4094a33a73246aae765ba517b2b))
* Removed cargo-workspace plugin from release-please-config ([5417990](https://github.com/Stephan-MC/ruvel/commit/54179900d28745141f86263041d08ab555d028ef))
* Removed unnecessary lines from release-please-configs ([2a07bf6](https://github.com/Stephan-MC/ruvel/commit/2a07bf653630cba7674d9ea54d6f82095fb16e9e))
* spawn tokio process in keep-alive mode ([f7d1e09](https://github.com/Stephan-MC/ruvel/commit/f7d1e09b86cd49c8c1effbbed9912d0f54a71733))


### Performance Improvements

* optimize throughput, memory allocation, and pipelining ([d483e7a](https://github.com/Stephan-MC/ruvel/commit/d483e7a1d89933c17e2f73ea89734de2aa42d2f8))

## [0.2.0](https://github.com/Stephan-MC/ruvel/compare/ruvel-v0.1.4...ruvel-v0.2.0) (2026-11-06)


### Features

* Added support for dynamic routing ([5d8e3ce](https://github.com/Stephan-MC/ruvel/commit/5d8e3ce2c657e7597ab85a020d235a9d8b95d673))
* Implemented Middleware ([286c17a](https://github.com/Stephan-MC/ruvel/commit/286c17a011be133c1444428cd098ead2c049ad8a))
* Updated memory allocations during keep-alive to tripple throughput ([3be46bb](https://github.com/Stephan-MC/ruvel/commit/3be46bb61adc6db3357a050d8b951211a46a0c55))
* Updated memory allocations during keep-alive to tripple throughput ([7cab6b6](https://github.com/Stephan-MC/ruvel/commit/7cab6b644c3e2871b67606d6e97d5b393f79f9e9))


### Bug Fixes

* Added release-please to enable automatic releases ([f860c0d](https://github.com/Stephan-MC/ruvel/commit/f860c0df63d1f22fc900c3001d21dfc31d98c0b7))
* Added release-please-token to ci ([7320ab9](https://github.com/Stephan-MC/ruvel/commit/7320ab9ff8437b34c9acc4b98f98cab158df9962))
* Address controller async-based warnings on terminal when running server ([12eb15c](https://github.com/Stephan-MC/ruvel/commit/12eb15cc719ab0dd1a13a1497f0a49c4d0fba9ef))
* Bumped to v0.1.4 ([4bf60e4](https://github.com/Stephan-MC/ruvel/commit/4bf60e445d8a16c78d29031bb62ab51d7d359c68))
* Bumped to v0.1.4 ([ac4adab](https://github.com/Stephan-MC/ruvel/commit/ac4adab00677c115871fdb6240c77e0fa86e521f))
* Bumped to verison 0.1.2 ([82f83e4](https://github.com/Stephan-MC/ruvel/commit/82f83e4d074bfc2499ead24b69da217aaef60a00))
* Bumped to verison 0.1.2 ([354422b](https://github.com/Stephan-MC/ruvel/commit/354422b3f684f4094a33a73246aae765ba517b2b))
* Removed cargo-workspace plugin from release-please-config ([5417990](https://github.com/Stephan-MC/ruvel/commit/54179900d28745141f86263041d08ab555d028ef))
* Removed unnecessary lines from release-please-configs ([2a07bf6](https://github.com/Stephan-MC/ruvel/commit/2a07bf653630cba7674d9ea54d6f82095fb16e9e))
* spawn tokio process in keep-alive mode ([f7d1e09](https://github.com/Stephan-MC/ruvel/commit/f7d1e09b86cd49c8c1effbbed9912d0f54a71733))


### Performance Improvements

* optimize throughput, memory allocation, and pipelining ([d483e7a](https://github.com/Stephan-MC/ruvel/commit/d483e7a1d89933c17e2f73ea89734de2aa42d2f8))
