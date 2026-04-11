#!/bin/bash

CUTOFF=${2:-1} 

grackle-ramparils "{\"direct\": true, \"cores\": 1, \"cls\": \"solverpy_grackle.runner.eprover.EproverRunner\", \"prefix\": \"eprover-\", \"penalty\": 10000000, \"domain1\": \"solverpy_grackle.trainer.eprover.default.DefaultDomain\", \"timeout\": $CUTOFF, \"penalty.error\": 10000000000}" $@ 

