#!/usr/bin/env bash

dfx start --clean --background

dfx deploy

cd tests/e2e && npm run e2e

dfx stop