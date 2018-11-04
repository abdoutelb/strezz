#!/bin/bash

pip install locustio &&

$0 node index.js &&

locust -f locustfile.py --host=$0 -P 3000
