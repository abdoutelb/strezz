FROM ubuntu:20.04

RUN  apt-get update  && apt-get  install -y apt-transport-https \
    && apt-get  install -y curl \
    && apt-get  install -y python3-pip python3-dev build-essential \
    &&  pip3 install locust
# Bundle app source

RUN curl --silent --location https://deb.nodesource.com/setup_14.x | bash -
RUN apt-get install --yes nodejs

# this is the build args parameter should be the same name
ARG url    
ENV SITE_URL=${url}

COPY /index.js .
COPY /helpers.js .

RUN URL=$SITE_URL node index.js

EXPOSE 4000

# this should be always simple string
CMD locust -f /loc.py --host=$SITE_URL -P 4000

#TO buld the images  docker build -t stress --build-arg url=URL .
#TO run the container docker run -itd -p 4000:4000 -label stress:latest
